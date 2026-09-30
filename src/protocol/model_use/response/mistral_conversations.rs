//! Mistral Conversations response wire.
//!
//! Conversions:
//! - Every `outputs` entry becomes a message: `message.output` an `Assistant` message, with its
//!   `thinking` chunks hoisted into a `Reasoning` message ahead of it, and `function.call` a
//!   `ToolUse` message.
//! - Usage maps `prompt_tokens`, `completion_tokens` and `total_tokens`.
//!
//! Trade-offs:
//! - The wire reports no reason of its own, so the stop reason is this crate's reading of the
//!   output: a `function.call` in `outputs` means the turn stopped to have a tool run, and anything
//!   else is `Stop`.
//! - Entries this crate does not model are dropped: `tool.execution` (a connector the service ran
//!   itself) and `agent.handoff`, and any entry type a later version of the wire adds. So is
//!   `conversation_id`, since every call carries its own history, and `confirmation_status`, since a
//!   call is not ours to confirm.
//! - A `function.call` entry's `arguments` are an object or a string; a string has to be JSON to
//!   become `ToolUse.arguments`.
//! - Neither cache counter and no reasoning-token count has a spelling here, so they stay `None`.

use serde_json::Value;

use crate::protocol::error::Error;
use crate::protocol::model_use::mistral_chunks::{join_text_chunks, join_thinking_chunks};
use crate::protocol::model_use::tool::parse_tool_arguments;
use crate::protocol::{ContentBlock, Message, Response, StopReason, Usage};

pub fn decode(body: &Value) -> Result<Response, Error> {
  let outputs = body
    .get("outputs")
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed("response carries no outputs".to_owned()))?;
  let mut messages: Vec<Message> = Vec::new();
  let mut stop_reason = StopReason::Stop;
  for entry in outputs {
    match entry.get("type").and_then(Value::as_str) {
      Some("message.output") => decode_message_output(entry, &mut messages)?,
      Some("function.call") => {
        messages.push(decode_function_call(entry)?);
        stop_reason = StopReason::ToolUse;
      }
      _ => {}
    }
  }
  Ok(Response { messages, stop_reason, usage: decode_usage(body), account_state: None })
}

/// One `message.output` entry: a thought becomes its own message ahead of the text, the order every
/// other wire in this crate puts them in.
fn decode_message_output(entry: &Value, messages: &mut Vec<Message>) -> Result<(), Error> {
  let chunks = match entry.get("content") {
    Some(Value::String(text)) => {
      if !text.is_empty() {
        messages.push(Message::Assistant {
          metadata: Default::default(),
          content: vec![ContentBlock::Text { text: text.clone() }],
        });
      }
      return Ok(());
    }
    Some(Value::Array(chunks)) => chunks,
    _ => {
      return Err(Error::Malformed(
        "message.output content is neither a string nor a chunk list".to_owned(),
      ));
    }
  };
  let plaintext = join_thinking_chunks(chunks)?;
  if !plaintext.is_empty() {
    messages.push(Message::reasoning_text(plaintext));
  }
  let text = join_text_chunks(chunks);
  if !text.is_empty() {
    messages.push(Message::Assistant {
      metadata: Default::default(),
      content: vec![ContentBlock::Text { text }],
    });
  }
  Ok(())
}

fn decode_function_call(entry: &Value) -> Result<Message, Error> {
  let call_id = entry
    .get("tool_call_id")
    .and_then(Value::as_str)
    .ok_or_else(|| Error::Malformed("function.call entry is missing `tool_call_id`".to_owned()))?;
  let name = entry
    .get("name")
    .and_then(Value::as_str)
    .ok_or_else(|| Error::Malformed("function.call entry is missing `name`".to_owned()))?;
  let arguments = match entry.get("arguments") {
    Some(Value::Object(arguments)) => Value::Object(arguments.clone()),
    Some(Value::String(raw)) => parse_tool_arguments(raw)
      .map_err(|_| Error::Malformed("function.call arguments are not valid JSON".to_owned()))?,
    _ => Value::Object(Default::default()),
  };
  Ok(Message::ToolUse {
    metadata: Default::default(),
    call_id: call_id.to_owned(),
    name: name.to_owned(),
    arguments,
  })
}

/// Maps the top-level `usage` object; shared with the stream decoder, whose terminal event carries
/// the same object.
pub(crate) fn decode_usage(body: &Value) -> Usage {
  let usage = body.get("usage");
  let field = |name: &str| usage.and_then(|usage| usage.get(name)).and_then(Value::as_u64);
  Usage {
    input_tokens: field("prompt_tokens"),
    cached_input_tokens: None,
    cache_write_input_tokens: None,
    output_tokens: field("completion_tokens"),
    reasoning_tokens: None,
    total_tokens: field("total_tokens"),
  }
}
