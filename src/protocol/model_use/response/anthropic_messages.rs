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
use crate::protocol::{ContentBlock, Message, Response, StopReason, Usage};
use serde_json::{Value, json};

pub fn decode(body: &Value) -> Result<Response, Error> {
  if body.get("type").and_then(Value::as_str) == Some("error") {
    let error = body.get("error");
    let message = error
      .and_then(|error| error.get("message"))
      .and_then(Value::as_str)
      .unwrap_or("upstream reported an error without a message");
    let code = error.and_then(|error| error.get("type")).and_then(Value::as_str);
    return Err(Error::in_band(code.map(str::to_owned), message.to_owned()));
  }

  let mut messages: Vec<Message> = Vec::new();
  let mut content: Vec<ContentBlock> = Vec::new();
  let flush = |messages: &mut Vec<Message>, content: &mut Vec<ContentBlock>| {
    if content.is_empty() {
      return;
    }
    messages.push(Message::Assistant { content: std::mem::take(content) });
  };
  if let Some(blocks) = body.get("content").and_then(Value::as_array) {
    for block in blocks {
      match block.get("type").and_then(Value::as_str) {
        Some("text") => content.push(ContentBlock::Text {
          text: block.get("text").and_then(Value::as_str).unwrap_or("").to_owned(),
        }),
        Some("thinking") => {
          flush(&mut messages, &mut content);
          let thinking = block.get("thinking").and_then(Value::as_str).unwrap_or("");
          messages.push(Message::Reasoning {
            plaintext: thinking.to_owned(),
            display: thinking.to_owned(),
            signature: block.get("signature").and_then(Value::as_str).unwrap_or("").to_owned(),
            ciphertext: String::new(),
          });
        }
        Some("redacted_thinking") => {
          flush(&mut messages, &mut content);
          messages.push(Message::Reasoning {
            plaintext: String::new(),
            display: String::new(),
            signature: String::new(),
            ciphertext: block.get("data").and_then(Value::as_str).unwrap_or("").to_owned(),
          });
        }
        Some("tool_use") => {
          flush(&mut messages, &mut content);
          messages.push(decode_tool_use(block)?);
        }
        _ => {}
      }
    }
  }
  flush(&mut messages, &mut content);
  Ok(Response {
    messages,
    stop_reason: stop_reason(body.get("stop_reason").and_then(Value::as_str)),
    usage: decode_usage(body),
    account_state: None,
  })
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
  Ok(Message::ToolUse { call_id: call_id.to_owned(), name: name.to_owned(), arguments })
}

/// Maps one wire stop reason; shared with the stream decoder so both paths agree.
pub fn stop_reason(reason: Option<&str>) -> StopReason {
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
  let usage = body.get("usage");
  let field =
    |name: &str| -> Option<u64> { usage.and_then(|usage| usage.get(name)).and_then(Value::as_u64) };
  let cached = field("cache_read_input_tokens");
  let cache_write = field("cache_creation_input_tokens");
  let input = field("input_tokens").map(|input| {
    input.saturating_add(cached.unwrap_or(0)).saturating_add(cache_write.unwrap_or(0))
  });
  let output = field("output_tokens");
  let total = input.zip(output).map(|(input, output)| input.saturating_add(output));
  Usage {
    input_tokens: input,
    cached_input_tokens: cached,
    cache_write_input_tokens: cache_write,
    output_tokens: output,
    reasoning_tokens: usage
      .and_then(|usage| usage.pointer("/output_tokens_details/thinking_tokens"))
      .and_then(Value::as_u64),
    total_tokens: total,
  }
}
