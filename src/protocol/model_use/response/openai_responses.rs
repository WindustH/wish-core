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

use crate::protocol::error::Error;
use crate::protocol::ReasoningOpaqueKind;
use crate::protocol::{ContentBlock, Message, Response, StopReason, Usage};
use serde_json::{Value, json};

pub fn decode(body: &Value) -> Result<Response, Error> {
  if body.get("status").and_then(Value::as_str) == Some("failed") {
    return Err(decode_upstream_error(body));
  }
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
        Some("function_call")
          if decode_stop_reason(body, false) != StopReason::MaxOutputLengthExceeded =>
        {
          messages.push(decode_function_call(item)?)
        }
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

pub(crate) fn decode_upstream_error(body: &Value) -> Error {
  let error = body.get("error");
  let message = error
    .and_then(|error| error.get("message"))
    .and_then(Value::as_str)
    .unwrap_or("upstream reported failure without an error message");
  let code = error.and_then(|error| error.get("code")).and_then(Value::as_str);
  Error::from_in_band(code.map(str::to_owned), message.to_owned())
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
  let arguments = match item.get("arguments").and_then(Value::as_str) {
    Some(raw) if !raw.trim().is_empty() => serde_json::from_str(raw)
      .map_err(|_| Error::Malformed("function_call arguments are not valid JSON".to_owned()))?,
    _ => json!({}),
  };
  Ok(Message::ToolUse {
    metadata: Default::default(),
    call_id: call_id.to_owned(),
    name: name.to_owned(),
    arguments,
  })
}

fn decode_stop_reason(body: &Value, has_tool_uses: bool) -> StopReason {
  map_stop_reason(body, has_tool_uses)
}

pub(crate) fn map_stop_reason(body: &Value, has_tool_uses: bool) -> StopReason {
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

fn decode_usage(body: &Value) -> Usage {
  parse_usage(body)
}

pub(crate) fn parse_usage(body: &Value) -> Usage {
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
