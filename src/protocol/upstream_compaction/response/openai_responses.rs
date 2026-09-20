//! OpenAI Responses compaction wire, shared by the platform API and the Codex deployment.
//!
//! Conversions:
//! - `output[]` is the conversation to continue with, read item by item: `compaction` -> the opaque
//!   `Message::UpstreamCompaction` standing in for the history, `message` -> the role it names (`user`,
//!   `assistant`, `system`, `developer`), `function_call` -> `ToolUse`, `function_call_output` ->
//!   `ToolResult` with an empty name because the wire carries none, and `reasoning` -> `Reasoning`
//!   the way a model call's reply reads it.
//! - `usage` reads the counts the wire reports for the compaction itself.
//!
//! Constraints:
//! - A `compaction` item without its payload is an error, not an empty compaction: that payload is
//!   the whole history, and losing it quietly would leave a conversation that looks complete.
//! - An item this reader cannot represent is dropped and named in `warnings`, so a caller can tell a
//!   faithfully read conversation from a lossy one.
//!
//! Trade-offs:
//! - `status: "failed"` is reported as `Error::Upstream` with the reply's `error.code`, the way a
//!   model call reports it.
//! - `dropped_message_count` is not modeled: it counts what was folded in, which the caller can
//!   count from the history it sent.

use crate::protocol::error::Error;
use crate::protocol::model_use::message::{ContentBlock, Conversation, Message};
use crate::protocol::model_use::response::openai_responses as model_use;
use crate::protocol::upstream_compaction::UpstreamCompaction;
use serde_json::Value;

pub fn decode(body: &Value) -> Result<UpstreamCompaction, Error> {
  if body.get("status").and_then(Value::as_str) == Some("failed") {
    return Err(model_use::decode_upstream_error(body));
  }
  let mut conversation: Conversation = Vec::new();
  let mut warnings: Vec<String> = Vec::new();
  for item in body.get("output").and_then(Value::as_array).into_iter().flatten() {
    match item.get("type").and_then(Value::as_str) {
      // The item stands in for the history: the payload goes back exactly as it came, so it is kept
      // as it is and never read here.
      Some("compaction") => {
        let (id, encrypted_content) = decode_compaction_payload(item)?;
        conversation.push(Message::UpstreamCompaction {
          metadata: Default::default(),
          id,
          encrypted_content,
        });
      }
      Some("message") => match decode_message(item) {
        Ok(message) => conversation.push(message),
        Err(why) => warnings.push(why),
      },
      Some("reasoning") => {
        if let Some(message) = model_use::decode_reasoning(item) {
          conversation.push(message);
        }
      }
      Some("function_call") => conversation.push(model_use::decode_function_call(item)?),
      Some("function_call_output") => conversation.push(decode_function_output(item)?),
      Some(other) => warnings.push(format!("an `{other}` item was dropped")),
      None => warnings.push("an item without a `type` was dropped".to_owned()),
    }
  }
  Ok(UpstreamCompaction {
    conversation,
    usage: model_use::parse_usage(body),
    account_state: None,
    warnings,
  })
}

/// The service's own two parts of that item: its name for the compaction when it gave one, and the
/// opaque payload it stands in for the history with.
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

/// One message item, or why it cannot become one of our messages.
fn decode_message(item: &Value) -> Result<Message, String> {
  let role = item.get("role").and_then(Value::as_str).unwrap_or("");
  let build: fn(Vec<ContentBlock>) -> Message = match role {
    "user" => |content| Message::User { metadata: Default::default(), content },
    "assistant" => |content| Message::Assistant { metadata: Default::default(), content },
    "system" => |content| Message::System { metadata: Default::default(), content },
    "developer" => |content| Message::Developer { metadata: Default::default(), content },
    other => return Err(format!("a `message` item with role `{other}` was dropped")),
  };
  let content: Vec<ContentBlock> = item
    .get("content")
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .filter_map(|part| match part.get("type").and_then(Value::as_str) {
      Some("input_text" | "output_text") => Some(ContentBlock::Text {
        text: part.get("text").and_then(Value::as_str).unwrap_or("").to_owned(),
      }),
      _ => None,
    })
    .collect();
  if content.is_empty() {
    return Err(format!("a `message` item with role `{role}` carries no text and was dropped"));
  }
  Ok(build(content))
}

/// One tool result: the call it answers, and its payload as it came.
fn decode_function_output(item: &Value) -> Result<Message, Error> {
  let call_id = item
    .get("call_id")
    .and_then(Value::as_str)
    .ok_or_else(|| Error::Malformed("function_call_output item is missing `call_id`".to_owned()))?;
  Ok(Message::ToolResult {
    metadata: Default::default(),
    call_id: call_id.to_owned(),
    name: String::new(),
    content: item.get("output").cloned().unwrap_or(Value::Null),
  })
}
