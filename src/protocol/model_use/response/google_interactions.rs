//! Google `interactions` response wire.
//!
//! Conversions:
//! - `steps[]` decodes into our model: `thought` -> `Reasoning`, `function_call` -> `ToolUse` and
//!   `model_output` -> `Assistant`. The echoed `user_input` and `function_result` steps are ignored.
//! - Thought text is read from `summary[].text` first and falls back to the `content` string; the
//!   step's `signature` becomes the thought's own proof.
//! - `status` maps `incomplete` / `budget_exceeded` to `MaxOutputLengthExceeded`, `requires_action`
//!   to `ToolUse` and `cancelled` to `Cancelled`; what a `completed` turn means is in the
//!   trade-offs.
//!
//! Trade-offs:
//! - A `completed` turn carries no reason of its own, so it is read as `ToolUse` when a call was made
//!   and `Stop` otherwise.
//! - A `failed` status is reported as an upstream error (with the resource's own `error` where it has
//!   one) rather than a `StopReason`, because such a resource carries no usable output.
//! - Text summary blocks are joined for display; the original thought step is retained separately
//!   so its signed text and image summary parts can be replayed unchanged.
//! - Usage names differ between wire generations (`candidates_token_count` / `total_output_tokens` /
//!   `completion_tokens`, and the `total_*` counters), and thinking is reported beside the
//!   candidates rather than inside them: it is folded into `output_tokens` the way it is billed.
//! - A call is read from `name` + `arguments`, or from `tool_name` + `args` in older traffic, and
//!   errors map to `Error::from_in_band` with `error.status` as the code.

use crate::protocol::error::Error;
use crate::protocol::ReasoningOpaqueKind;
use crate::protocol::{ContentBlock, Message, Response, StopReason, Usage};
use serde_json::{Value, json};

pub fn decode(body: &Value) -> Result<Response, Error> {
  if let Some(error) = body.get("error") {
    return Err(decode_error_object(error));
  }
  if body.get("status").and_then(Value::as_str) == Some("failed") {
    return Err(decode_failed_error(body));
  }

  let mut messages: Vec<Message> = Vec::new();
  if let Some(steps) = body.get("steps").and_then(Value::as_array) {
    for step in steps {
      match step.get("type").and_then(Value::as_str) {
        Some("thought") => messages.push(decode_thought(step)),
        Some("function_call") => messages.push(decode_function_call(step)?),
        Some("model_output") => {
          let content = decode_blocks(step.get("content"));
          if !content.is_empty() {
            messages.push(Message::Assistant { metadata: Default::default(), content });
          }
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

fn decode_thought(step: &Value) -> Message {
  let mut text: Vec<&str> = Vec::new();
  if let Some(summary) = step.get("summary").and_then(Value::as_array) {
    for entry in summary {
      if let Some(entry_text) = entry.get("text").and_then(Value::as_str) {
        text.push(entry_text);
      }
    }
  }
  if text.is_empty()
    && let Some(content) = step.get("content").and_then(Value::as_str)
  {
    text.push(content);
  }
  let plaintext = text.join("\n");
  Message::Reasoning {
    metadata: Default::default(),
    replay_item: Some(step.clone()),
    opaque_kind: Some(ReasoningOpaqueKind::GoogleInteractionsThought),
    plaintext: plaintext.clone(),
    display: plaintext,
    signature: step.get("signature").and_then(Value::as_str).unwrap_or("").to_owned(),
    ciphertext: String::new(),
  }
}

fn decode_function_call(step: &Value) -> Result<Message, Error> {
  let name = step
    .get("name")
    .or_else(|| step.get("tool_name"))
    .and_then(Value::as_str)
    .ok_or_else(|| Error::Malformed("function_call step is missing `name`".to_owned()))?;
  let call_id = step.get("id").and_then(Value::as_str).unwrap_or("");
  let arguments =
    step.get("arguments").or_else(|| step.get("args")).cloned().unwrap_or_else(|| json!({}));
  Ok(Message::ToolUse {
    metadata: Default::default(),
    call_id: call_id.to_owned(),
    name: name.to_owned(),
    arguments,
  })
}

fn decode_blocks(content: Option<&Value>) -> Vec<ContentBlock> {
  match content {
    Some(Value::String(text)) => {
      if text.is_empty() {
        Vec::new()
      } else {
        vec![ContentBlock::Text { text: text.clone() }]
      }
    }
    Some(Value::Array(blocks)) => blocks.iter().filter_map(decode_block).collect(),
    _ => Vec::new(),
  }
}

fn decode_block(block: &Value) -> Option<ContentBlock> {
  match block.get("type").and_then(Value::as_str) {
    Some("text") => {
      Some(ContentBlock::Text { text: block.get("text").and_then(Value::as_str)?.to_owned() })
    }
    Some("image") => Some(ContentBlock::Image {
      mime_type: block.get("mime_type").and_then(Value::as_str)?.to_owned(),
      data_base64: block.get("data").and_then(Value::as_str)?.to_owned(),
    }),
    _ => None,
  }
}

/// The failure a `failed` interaction reports: the error object it carries where it has one, and a
/// plain statement of the status otherwise.
pub(crate) fn decode_failed_error(body: &Value) -> Error {
  match body.get("error") {
    Some(error) => decode_error_object(error),
    None => Error::from_in_band(None, "the interaction ended in failure".to_owned()),
  }
}

fn decode_error_object(error: &Value) -> Error {
  let message = error
    .get("message")
    .and_then(Value::as_str)
    .unwrap_or("upstream reported an error without a message");
  let code = error.get("status").and_then(Value::as_str);
  Error::from_in_band(code.map(str::to_owned), message.to_owned())
}

fn decode_stop_reason(body: &Value, has_tool_uses: bool) -> StopReason {
  map_stop_reason(body.get("status").and_then(Value::as_str), has_tool_uses)
}

pub(crate) fn map_stop_reason(status: Option<&str>, has_tool_uses: bool) -> StopReason {
  match status {
    Some("completed") => {
      if has_tool_uses {
        StopReason::ToolUse
      } else {
        StopReason::Stop
      }
    }
    Some("incomplete" | "budget_exceeded") => StopReason::MaxOutputLengthExceeded,
    // The server paused waiting for tool results: function_call steps are outstanding.
    Some("requires_action") => StopReason::ToolUse,
    // A `failed` status never reaches this mapping: `decode` reports it as an upstream error.
    Some("cancelled") => StopReason::Cancelled,
    _ => StopReason::Unknown,
  }
}

fn decode_usage(body: &Value) -> Usage {
  parse_usage(body)
}

/// Maps the top-level `usage` object.
///
/// Three naming generations are accepted: the counts the spec documents (`prompt_token_count`,
/// `candidates_token_count`, `total_output_tokens`), the `total_*` counters the preview used, and
/// the OpenAI-style pair. Thinking is reported beside the candidates rather than inside them, and
/// is folded into `output_tokens` the way it is billed.
pub(crate) fn parse_usage(body: &Value) -> Usage {
  let usage = body.get("usage");
  let field = |names: &[&str]| -> Option<u64> {
    usage.and_then(|usage| names.iter().find_map(|name| usage.get(name))).and_then(Value::as_u64)
  };
  let candidates = field(&["candidates_token_count", "total_output_tokens", "completion_tokens"]);
  let thoughts = field(&["total_thought_tokens"]);
  Usage {
    input_tokens: field(&["prompt_token_count", "total_input_tokens", "prompt_tokens"]),
    cached_input_tokens: field(&["total_cached_tokens"]),
    cache_write_input_tokens: None,
    output_tokens: match (candidates, thoughts) {
      (Some(candidates), Some(thoughts)) => Some(candidates + thoughts),
      (candidates, thoughts) => candidates.or(thoughts),
    },
    reasoning_tokens: thoughts,
    total_tokens: field(&["total_token_count", "total_tokens"]),
  }
}
