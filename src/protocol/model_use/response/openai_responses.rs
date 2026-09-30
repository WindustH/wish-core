//! OpenAI Responses response wire.
//!
//! Conversions:
//! - `output[]` decodes in order: `reasoning` -> `Reasoning` (encrypted blob into `ciphertext`,
//!   `content[].reasoning_text` into `plaintext`, `summary[].summary_text` into `display`),
//!   `message` -> `Assistant` (from `output_text` parts) and `function_call` -> `ToolUse`.
//! - `status` maps `completed` to `ToolUse` when a call was made and to `Stop` otherwise, while
//!   `incomplete` reads `incomplete_details.reason` (`max_output_tokens` ->
//!   `MaxOutputLengthExceeded`, `max_messages` -> `MaxMessages`, `content_filter` ->
//!   `ContentFilter`, `tool_calls` -> `ToolUse`, `steered` -> `Steered`); `cancelled` maps to
//!   `Cancelled`.
//! - Usage reads the cached and cache write counters and `output_tokens_details.reasoning_tokens`.
//!
//! Trade-offs:
//! - A `failed` status is reported as `Error::Upstream` (with `error.code`) instead of a
//!   `StopReason`, because such a body carries no usable output.
//! - A `compaction` item in a buffered reply is skipped, while the stream decoder hands one over
//!   whole as [`StreamEvent::UpstreamCompaction`](crate::protocol::StreamEvent::UpstreamCompaction)
//!   and the compaction call's own reader keeps it too; those two read its payload through
//!   [`decode_compaction_payload`].

use crate::protocol::error::Error;
use crate::protocol::http_error::decode_in_band;
use crate::protocol::model_use::tool::parse_tool_arguments;
use crate::protocol::{ContentBlock, Message, ReasoningOpaqueKind, Response, StopReason, Usage};
use serde_json::Value;

pub fn decode(body: &Value) -> Result<Response, Error> {
  if body.get("status").and_then(Value::as_str) == Some("failed") {
    return Err(decode_in_band_error(body));
  }
  // The calls of a capped reply may be cut short, so they are not read at all.
  let capped = decode_stop_reason(body, false) == StopReason::MaxOutputLengthExceeded;
  let mut messages: Vec<Message> = Vec::new();
  if let Some(output) = body.get("output").and_then(Value::as_array) {
    for item in output {
      match item.get("type").and_then(Value::as_str) {
        Some("reasoning") => {
          if let Some(message) = decode_reasoning(item) {
            messages.push(message);
          }
        }
        Some("message") => {
          let content = decode_message_content(item);
          if !content.is_empty() {
            messages.push(Message::Assistant { metadata: Default::default(), content });
          }
        }
        Some("function_call") if !capped => messages.push(decode_function_call(item)?),
        _ => {}
      }
    }
  }
  let has_tool_uses = messages.iter().any(|message| matches!(message, Message::ToolUse { .. }));
  Ok(Response {
    messages,
    stop_reason: decode_stop_reason(body, has_tool_uses),
    usage: decode_usage(body),
    account_state: None,
  })
}

/// The failure a `failed` response reports, from its `error` object.
pub(crate) fn decode_in_band_error(body: &Value) -> Error {
  decode_in_band(body.get("error"), &["code"], "upstream reported failure without an error message")
}

pub(crate) fn decode_reasoning(item: &Value) -> Option<Message> {
  let join_typed_text = |entries: Option<&Value>, kind: &str| -> String {
    let Some(entries) = entries.and_then(Value::as_array) else {
      return String::new();
    };
    entries
      .iter()
      .filter(|entry| entry.get("type").and_then(Value::as_str) == Some(kind))
      .filter_map(|entry| entry.get("text").and_then(Value::as_str))
      .collect::<Vec<_>>()
      .join("\n")
  };
  let ciphertext = item.get("encrypted_content").and_then(Value::as_str).unwrap_or("");
  let plaintext = join_typed_text(item.get("content"), "reasoning_text");
  let display = join_typed_text(item.get("summary"), "summary_text");
  if ciphertext.is_empty() && plaintext.is_empty() && display.is_empty() {
    return None;
  }
  Some(Message::Reasoning {
    metadata: Default::default(),
    replay_item: None,
    opaque_kind: (!ciphertext.is_empty()).then_some(ReasoningOpaqueKind::OpenAiEncrypted),
    plaintext,
    display,
    signature: String::new(),
    ciphertext: ciphertext.to_owned(),
  })
}

fn decode_message_content(item: &Value) -> Vec<ContentBlock> {
  let Some(parts) = item.get("content").and_then(Value::as_array) else {
    return Vec::new();
  };
  parts
    .iter()
    .filter_map(|part| match part.get("type").and_then(Value::as_str) {
      Some("output_text") => Some(ContentBlock::Text {
        text: part.get("text").and_then(Value::as_str).unwrap_or("").to_owned(),
      }),
      _ => None,
    })
    .collect()
}

pub(crate) fn decode_function_call(item: &Value) -> Result<Message, Error> {
  let field = |key: &str| {
    item
      .get(key)
      .and_then(Value::as_str)
      .ok_or_else(|| Error::Malformed(format!("function_call item is missing `{key}`")))
  };
  let call_id = field("call_id")?;
  let name = field("name")?;
  let raw = item.get("arguments").and_then(Value::as_str).unwrap_or_default();
  let arguments = parse_tool_arguments(raw)
    .map_err(|_| Error::Malformed("function_call arguments are not valid JSON".to_owned()))?;
  Ok(Message::ToolUse {
    metadata: Default::default(),
    call_id: call_id.to_owned(),
    name: name.to_owned(),
    arguments,
  })
}

/// The two parts of the item a compacted history travels as: the service's name for the
/// compaction when it gave one, and the opaque payload that stands in for the history.
///
/// A `compaction` item without its payload is an error, not an empty compaction: that payload is
/// the whole history, and losing it quietly would leave a conversation that looks complete.
pub(crate) fn decode_compaction_payload(item: &Value) -> Result<(Option<String>, String), Error> {
  let payload = item
    .get("encrypted_content")
    .and_then(Value::as_str)
    .filter(|payload| !payload.is_empty())
    .ok_or_else(|| {
      Error::Malformed("a `compaction` item carries no `encrypted_content`".to_owned())
    })?;
  Ok((item.get("id").and_then(Value::as_str).map(str::to_owned), payload.to_owned()))
}

/// Maps a response's `status` (and, when it is `incomplete`, its reason); shared with the stream
/// decoder, whose terminal event carries the same response. A completed response that made calls
/// stopped for them.
pub(crate) fn decode_stop_reason(body: &Value, has_tool_uses: bool) -> StopReason {
  match body.get("status").and_then(Value::as_str) {
    Some("completed") => {
      if has_tool_uses {
        StopReason::ToolUse
      } else {
        StopReason::Stop
      }
    }
    Some("incomplete") => {
      match body.pointer("/incomplete_details/reason").and_then(Value::as_str) {
        Some("max_output_tokens") => StopReason::MaxOutputLengthExceeded,
        Some("max_messages") => StopReason::MaxMessages,
        Some("content_filter") => StopReason::ContentFilter,
        Some("tool_calls") => StopReason::ToolUse,
        Some("steered") => StopReason::Steered,
        _ => StopReason::Unknown,
      }
    }
    Some("cancelled") => StopReason::Cancelled,
    _ => StopReason::Unknown,
  }
}

/// Maps the response's `usage` object; shared with the stream decoder and the compaction reader.
pub(crate) fn decode_usage(body: &Value) -> Usage {
  let usage = body.get("usage");
  let field = |path: &[&str]| -> Option<u64> {
    let mut node = usage?;
    for key in path {
      node = node.get(*key)?;
    }
    node.as_u64()
  };
  Usage {
    input_tokens: field(&["input_tokens"]),
    cached_input_tokens: field(&["input_tokens_details", "cached_tokens"]),
    cache_write_input_tokens: field(&["input_tokens_details", "cache_write_tokens"]),
    output_tokens: field(&["output_tokens"]),
    reasoning_tokens: field(&["output_tokens_details", "reasoning_tokens"]),
    total_tokens: field(&["total_tokens"]),
  }
}
