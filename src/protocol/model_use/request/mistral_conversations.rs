//! Mistral Conversations request wire.
//!
//! Conversions:
//! - The history travels in `inputs` as entries: a `User` or `Assistant` message is a
//!   `message.input` entry, a `ToolUse` a `function.call` entry and the `ToolResult` that answers it
//!   a `function.result` entry. A replayed thought rides in its assistant entry's `content` as a
//!   `thinking` chunk ahead of the text, the way the chat wire attaches it to that turn.
//! - `System` messages have no turn of their own here: their text merges, in conversation order,
//!   into the top-level `instructions` string.
//! - `max_output_tokens`, `tool_choice` and the reasoning effort go into `completion_args`; the tools
//!   themselves are top-level and only functions are declared.
//! - `ReasoningConfig` has one knob, `reasoning_effort`: `enabled` is spelled as the word at the
//!   appropriate end of that knob when the caller gave no word of their own, a caller's word is
//!   passed through as it stands, and a summary is rejected.
//!
//! Constraints:
//! - The wire keeps a conversation server-side: an entry is seen only if it is in this request's
//!   `inputs` or already in the conversation the request names, so a request that starts a
//!   conversation has to carry the history it wants considered.
//! - `inputs` is required and needs at least one entry.
//! - `instructions` only takes text: an instruction message carrying any other block is rejected.
//! - A `function.result` entry answers a `function.call` entry by `tool_call_id`.
//!
//! Trade-offs:
//! - Every call starts a fresh conversation with `store: false` and the whole history rather than
//!   appending to a stored one: a call stays reproducible from the request alone and leaves nothing
//!   behind to delete, at the price of sending the history again.
//! - `handoff_execution: "client"` is always sent, because built-in connectors are not modelled: a
//!   function call comes back to the caller instead of being answered by the service.
//! - A `Developer` message has no role of its own on this wire, so its text joins the same
//!   `instructions` string.
//! - `Request.cache` is ignored: this wire has no cache key and no breakpoints.
//! - Only function tools are declared, which leaves the built-in connector tools (web search, code
//!   interpreter, image generation, document library, custom connectors) without a spelling here.
//! - A `function.result` entry carries a string, so a payload that is not a string is stringified
//!   the way the chat wire stringifies one.
//! - `MessageInputEntry.prefix` is not modelled, because prefill is not.

use serde_json::{Map, Value, json};

use crate::protocol::error::Error;
use crate::protocol::{ContentBlock, Message, ReasoningConfig, Request, Tool, ToolChoice};

/// Headers every call carries, besides auth and `content-type`.
pub const HEADERS: &[(&str, &str)] = &[];

pub fn render(request: &Request) -> Result<Value, Error> {
  let mut body = Map::new();
  body.insert("model".into(), json!(request.model));
  // Nothing is kept server-side: the entries below are the whole conversation.
  body.insert("store".into(), json!(false));
  // Functions are ours to run, so the model has to stop and hand a call back.
  body.insert("handoff_execution".into(), json!("client"));
  if request.stream {
    body.insert("stream".into(), json!(true));
  }
  if let Some(instructions) = render_instructions(&request.conversation)? {
    body.insert("instructions".into(), json!(instructions));
  }
  if !request.tools.is_empty() {
    let tools: Vec<Value> = request.tools.iter().map(render_tool).collect();
    body.insert("tools".into(), Value::Array(tools));
  }
  let mut completion = Map::new();
  if let Some(max_output_tokens) = request.max_output_tokens {
    completion.insert("max_tokens".into(), json!(max_output_tokens));
  }
  if let Some(choice) = request.tool_choice {
    completion.insert("tool_choice".into(), json!(render_tool_choice(choice)));
  }
  render_reasoning(request.reasoning.as_ref(), &mut completion)?;
  if !completion.is_empty() {
    body.insert("completion_args".into(), Value::Object(completion));
  }
  body.insert("inputs".into(), Value::Array(render_inputs(&request.conversation)?));
  Ok(Value::Object(body))
}

/// The wire's one place for standing instructions: every `System` and `Developer` message, in
/// conversation order, joined into one string.
fn render_instructions(conversation: &[Message]) -> Result<Option<String>, Error> {
  let mut text = String::new();
  for message in conversation {
    let content = match message {
      Message::System { content, .. } | Message::Developer { content, .. } => content,
      _ => continue,
    };
    for block in content {
      let ContentBlock::Text { text: block_text } = block else {
        return Err(Error::Build(
          "a system or developer message can only carry text blocks on the conversations wire"
            .to_owned(),
        ));
      };
      if !text.is_empty() {
        text.push('\n');
      }
      text.push_str(block_text);
    }
  }
  Ok((!text.is_empty()).then_some(text))
}

/// The history as entries, in conversation order.
fn render_inputs(conversation: &[Message]) -> Result<Vec<Value>, Error> {
  let mut entries: Vec<Value> = Vec::new();
  let mut pending_reasoning: Option<String> = None;
  for message in conversation {
    match message {
      Message::System { .. } | Message::Developer { .. } => {}
      Message::UpstreamCompaction { .. } => {
        return Err(Error::Build(
          "the mistral conversations wire cannot carry a compacted conversation".to_owned(),
        ));
      }
      Message::User { content, .. } => {
        pending_reasoning = None;
        entries.push(render_message_input("user", content, None)?);
      }
      Message::Assistant { content, .. } => {
        entries.push(render_message_input("assistant", content, pending_reasoning.take())?);
      }
      Message::Reasoning { plaintext, .. } => {
        pending_reasoning.get_or_insert_with(String::new).push_str(plaintext);
      }
      Message::ToolUse { call_id, name, arguments, .. } => {
        pending_reasoning = None;
        entries.push(json!({
          "object": "entry",
          "type": "function.call",
          "tool_call_id": call_id,
          "name": name,
          "arguments": arguments.to_string(),
        }));
      }
      Message::ToolResult { call_id, content, .. } => {
        pending_reasoning = None;
        let result = match content {
          Value::String(text) => text.clone(),
          other => other.to_string(),
        };
        entries.push(json!({
          "object": "entry",
          "type": "function.result",
          "tool_call_id": call_id,
          "result": result,
        }));
      }
    }
  }
  if entries.is_empty() {
    return Err(Error::Build("a conversations request needs at least one input entry".to_owned()));
  }
  Ok(entries)
}

/// One `message.input` entry: text alone travels as a string, anything else as chunks, and a
/// replayed thought is the first chunk.
fn render_message_input(
  role: &str,
  content: &[ContentBlock],
  reasoning: Option<String>,
) -> Result<Value, Error> {
  let mut chunks: Vec<Value> = Vec::new();
  if let Some(reasoning) = reasoning.filter(|reasoning| !reasoning.is_empty()) {
    chunks.push(render_thinking_chunk(&reasoning));
  }
  let mut text: Vec<&str> = Vec::new();
  for block in content {
    match block {
      ContentBlock::Text { text: block_text } => text.push(block_text),
      ContentBlock::Image { mime_type, data_base64 } => {
        if role == "assistant" {
          return Err(Error::Build(
            "an assistant message cannot carry images on the conversations wire".to_owned(),
          ));
        }
        chunks.push(json!({
          "type": "image_url",
          "image_url": { "url": format!("data:{mime_type};base64,{data_base64}") },
        }));
      }
    }
  }
  let text = text.join("\n");
  let content = if chunks.is_empty() {
    json!(text)
  } else {
    if !text.is_empty() {
      chunks.push(json!({ "type": "text", "text": text }));
    }
    Value::Array(chunks)
  };
  Ok(json!({ "object": "entry", "type": "message.input", "role": role, "content": content }))
}

/// The chunk a replayed thought travels in on this wire.
fn render_thinking_chunk(reasoning: &str) -> Value {
  json!({
    "type": "thinking",
    "closed": true,
    "thinking": [{ "type": "text", "text": reasoning }],
  })
}

fn render_tool(tool: &Tool) -> Value {
  json!({
    "type": "function",
    "function": {
      "name": tool.name,
      "description": tool.description,
      "parameters": tool.input_schema,
    },
  })
}

fn render_tool_choice(choice: ToolChoice) -> &'static str {
  match choice {
    ToolChoice::Auto => "auto",
    ToolChoice::None => "none",
    // This wire's word for "at least one tool".
    ToolChoice::Required => "any",
  }
}

/// The wire's reasoning knob: one effort word, and the caller's on/off axis a word of its own.
fn render_reasoning(
  config: Option<&ReasoningConfig>,
  completion: &mut Map<String, Value>,
) -> Result<(), Error> {
  let Some(config) = config else { return Ok(()) };
  if config.summary.is_some() {
    return Err(Error::Build("the conversations wire has no reasoning summary axis".to_owned()));
  }
  let effort = match (config.enabled, config.effort.as_deref()) {
    (_, Some(effort)) => effort.to_owned(),
    (Some(true), None) => "high".to_owned(),
    (Some(false), None) => "none".to_owned(),
    (None, None) => return Ok(()),
  };
  completion.insert("reasoning_effort".into(), json!(effort.to_lowercase()));
  Ok(())
}
