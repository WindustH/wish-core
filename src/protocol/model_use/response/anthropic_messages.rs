//! Anthropic Messages response wire.
//!
//! Conversions:
//! - `content[]` decodes in order: `thinking` / `redacted_thinking` become `Reasoning` (text into
//!   `plaintext` / `display` with its `signature`, a redacted blob into `ciphertext`), `text`
//!   accumulates into an `Assistant` message, and every `tool_use` becomes its own `ToolUse` message.
//! - Assistant text is flushed before each reasoning block, so `Reasoning` ends up leading its
//!   assistant turn and the request side can split the same shape back apart.
//! - `stop_reason` maps `end_turn` / `stop_sequence` / `pause_turn` to `Stop`, `max_tokens` to
//!   `MaxOutputLengthExceeded`, `model_context_window_exceeded` to `ContextLengthExceeded`,
//!   `tool_use` to `ToolUse` and `refusal` to `ContentFilter`.
//!
//! Trade-offs:
//! - `input_tokens` is reported as the sum of input + cache read + cache write, because the wire
//!   splits the counters; the cache parts are preserved on their own fields. Thinking tokens come
//!   from `output_tokens_details`.
//! - The shape goes back as it came: a `redacted_thinking` blob lands in `ciphertext`, a thinking
//!   block's proof in `signature`, so a thinking block whose text was omitted stays a thinking
//!   block. Errors map to `Error::Upstream` with `error.type` as the code.

use crate::protocol::error::Error;
use crate::protocol::http_error::decode_in_band;
use crate::protocol::{ContentBlock, Message, ReasoningOpaqueKind, Response, StopReason, Usage};
use serde_json::{Value, json};

pub fn decode(body: &Value) -> Result<Response, Error> {
  if body.get("type").and_then(Value::as_str) == Some("error") {
    return Err(decode_in_band(
      body.get("error"),
      &["type"],
      "upstream reported an error without a message",
    ));
  }

  let stop_reason = map_stop_reason(body.get("stop_reason").and_then(Value::as_str));
  let mut messages: Vec<Message> = Vec::new();
  let mut content: Vec<ContentBlock> = Vec::new();
  let flush = |messages: &mut Vec<Message>, content: &mut Vec<ContentBlock>| {
    if content.is_empty() {
      return;
    }
    messages
      .push(Message::Assistant { metadata: Default::default(), content: std::mem::take(content) });
  };
  if let Some(blocks) = body.get("content").and_then(Value::as_array) {
    for block in blocks {
      let field = |name: &str| block.get(name).and_then(Value::as_str).unwrap_or("").to_owned();
      match block.get("type").and_then(Value::as_str) {
        Some("text") => content.push(ContentBlock::Text { text: field("text") }),
        Some("thinking") => {
          flush(&mut messages, &mut content);
          messages.push(Message::signed_reasoning(
            field("thinking"),
            field("signature"),
            ReasoningOpaqueKind::AnthropicSignature,
          ));
        }
        Some("redacted_thinking") => {
          flush(&mut messages, &mut content);
          messages.push(Message::redacted_reasoning(
            field("data"),
            ReasoningOpaqueKind::AnthropicRedacted,
          ));
        }
        // The calls of a capped reply may be cut short, so they are not read at all.
        Some("tool_use") if stop_reason != StopReason::MaxOutputLengthExceeded => {
          flush(&mut messages, &mut content);
          messages.push(decode_tool_use(block)?);
        }
        _ => {}
      }
    }
  }
  flush(&mut messages, &mut content);
  Ok(Response { messages, stop_reason, usage: decode_usage(body), account_state: None })
}

fn decode_tool_use(block: &Value) -> Result<Message, Error> {
  let call_id = block
    .get("id")
    .and_then(Value::as_str)
    .ok_or_else(|| Error::Malformed("tool_use block is missing `id`".to_owned()))?;
  let name = block
    .get("name")
    .and_then(Value::as_str)
    .ok_or_else(|| Error::Malformed("tool_use block is missing `name`".to_owned()))?;
  let arguments = block.get("input").cloned().unwrap_or_else(|| json!({}));
  Ok(Message::ToolUse {
    metadata: Default::default(),
    call_id: call_id.to_owned(),
    name: name.to_owned(),
    arguments,
  })
}

/// Maps one wire stop reason; shared with the stream decoder so both paths agree.
pub fn map_stop_reason(reason: Option<&str>) -> StopReason {
  match reason {
    Some("end_turn" | "stop_sequence" | "pause_turn") => StopReason::Stop,
    Some("max_tokens") => StopReason::MaxOutputLengthExceeded,
    Some("model_context_window_exceeded") => StopReason::ContextLengthExceeded,
    Some("tool_use") => StopReason::ToolUse,
    Some("refusal") => StopReason::ContentFilter,
    _ => StopReason::Unknown,
  }
}

fn decode_usage(body: &Value) -> Usage {
  let mut total = Usage::default();
  if let Some(usage) = body.get("usage") {
    merge_usage(&mut total, usage);
  }
  total
}

/// Merges one wire usage object into a running total: the counters it carries replace the ones
/// before, input is folded as `input_tokens` plus both cache counters, and the total is derived
/// once both sides are known.
///
/// A buffered body carries one usage object and a stream several, which the stream decoder merges
/// one after another into the total it re-emits; both paths read the counters the same way.
pub(crate) fn merge_usage(total: &mut Usage, usage: &Value) {
  let field = |name: &str| usage.get(name).and_then(Value::as_u64);
  let cached = field("cache_read_input_tokens");
  let cache_write = field("cache_creation_input_tokens");
  if let Some(cached) = cached {
    total.cached_input_tokens = Some(cached);
  }
  if let Some(cache_write) = cache_write {
    total.cache_write_input_tokens = Some(cache_write);
  }
  if let Some(input) = field("input_tokens") {
    total.input_tokens =
      Some(input.saturating_add(cached.unwrap_or(0)).saturating_add(cache_write.unwrap_or(0)));
  }
  if let Some(output) = field("output_tokens") {
    total.output_tokens = Some(output);
  }
  if let Some(thinking) =
    usage.pointer("/output_tokens_details/thinking_tokens").and_then(Value::as_u64)
  {
    total.reasoning_tokens = Some(thinking);
  }
  if let (Some(input), Some(output)) = (total.input_tokens, total.output_tokens) {
    total.total_tokens = Some(input.saturating_add(output));
  }
}
