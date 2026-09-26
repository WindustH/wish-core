//! OpenAI Chat Completions response wire.
//!
//! Conversions:
//! - Only `choices[0].message` is read: a non-empty `content` becomes an `Assistant` message and
//!   every `tool_calls` entry becomes its own `ToolUse` message (a missing id or name is an error).
//! - MiniMax refuses a call with `200` and `base_resp.status_code`, which becomes an in-band
//!   `Error::Upstream` rather than the missing-`choices` shape it would otherwise look like.
//! - Reasoning becomes a `Reasoning` message ahead of the assistant content: from the flat
//!   `reasoning_content` extension where the variant has it, or from the `thinking` chunks Mistral
//!   folds into `content`.
//! - `finish_reason` maps `stop` to `Stop`, `length` to `MaxOutputLengthExceeded`, `tool_calls` (or
//!   the older `function_call`) to `ToolUse` and `content_filter` (or a vendor's `sensitive`) to
//!   `ContentFilter`; Mistral's `model_length` and `model_context_window_exceeded` are both
//!   `ContextLengthExceeded`, and anything else is `Unknown`.
//! - Usage maps `prompt_tokens` / `completion_tokens` / `total_tokens` and picks up
//!   `prompt_tokens_details.cached_tokens` plus `completion_tokens_details.reasoning_tokens`.
//!
//! Trade-offs:
//! - The wire reports no cache write counter, so `cache_write_input_tokens` stays `None`; the
//!   message `refusal` field is dropped for now, and errors become `Error::Upstream` with
//!   `error.code` as the code.
//! - `thinking` chunks are hoisted into one `Reasoning` message ahead of the text, so where a thought
//!   sat between two text chunks is not kept; a `think` chunk is read as well as `thinking`, because
//!   the vendor's schema and its prose spell the type differently.

use crate::protocol::error::Error;
use crate::protocol::model_use::request::openai_chat::{
  ChatCompletionApiCompatMode, REASONING_FIELD,
};
use crate::protocol::{ContentBlock, Message, Response, StopReason, Usage};
use serde_json::{Value, json};

pub fn decode(body: &Value, mode: ChatCompletionApiCompatMode) -> Result<Response, Error> {
  if let Some(error) = body.get("error") {
    let message = error
      .get("message")
      .and_then(Value::as_str)
      .unwrap_or("upstream reported an error without a message");
    let code = error.get("code").and_then(Value::as_str);
    return Err(Error::from_in_band(code.map(str::to_owned), message.to_owned()));
  }

  // MiniMax refuses a call with `200` and its own envelope, and no `choices` at all.
  if mode == ChatCompletionApiCompatMode::MiniMax
    && let Some(error) = decode_refusal_error(body)
  {
    return Err(error);
  }

  let choice = body
    .pointer("/choices/0")
    .ok_or_else(|| Error::Malformed("response carries no choices".to_owned()))?;
  let message = choice
    .get("message")
    .ok_or_else(|| Error::Malformed("choice carries no message".to_owned()))?;

  let mut content: Vec<ContentBlock> = Vec::new();
  match message.get("content") {
    Some(Value::String(text)) if !text.is_empty() => {
      content.push(ContentBlock::Text { text: text.to_owned() });
    }
    Some(Value::Array(chunks)) if mode.is_mistral() => {
      let text: String = chunks
        .iter()
        .filter(|chunk| get_chunk_type(chunk) == Some("text"))
        .filter_map(|chunk| chunk.get("text").and_then(Value::as_str))
        .collect();
      if !text.is_empty() {
        content.push(ContentBlock::Text { text });
      }
    }
    _ => {}
  }
  let mut tool_uses: Vec<Message> = Vec::new();
  if decode_stop_reason(choice, mode) != StopReason::MaxOutputLengthExceeded
    && let Some(calls) = message.get("tool_calls").and_then(Value::as_array)
  {
    for call in calls {
      tool_uses.push(decode_tool_use(call)?);
    }
  }

  let mut messages: Vec<Message> = Vec::new();
  if let Some(reasoning) = decode_reasoning(message, mode)? {
    messages.push(reasoning);
  } else if let Some(reasoning) = decode_thinking_chunks(message, mode)? {
    messages.push(reasoning);
  }
  if !content.is_empty() {
    messages.push(Message::Assistant { metadata: Default::default(), content });
  }
  messages.extend(tool_uses);
  Ok(Response {
    messages,
    stop_reason: decode_stop_reason(choice, mode),
    usage: decode_usage(body),
    account_state: None,
  })
}

/// Captures the assistant message's reasoning, when the mode speaks the extension.
fn decode_reasoning(
  message: &Value,
  mode: ChatCompletionApiCompatMode,
) -> Result<Option<Message>, Error> {
  // Only the flat `reasoning_content` field carries reasoning here: the official wire has no such
  // field, and Mistral spells its reasoning in `content` chunks instead.
  if mode.is_plain() || mode.is_mistral() {
    return Ok(None);
  }
  let name = REASONING_FIELD;
  let Some(value) = message.get(name).filter(|value| !value.is_null()) else { return Ok(None) };
  let text = value.as_str().ok_or_else(|| Error::Malformed(format!("`{name}` is not a string")))?;
  if text.is_empty() {
    return Ok(None);
  }
  Ok(Some(Message::Reasoning {
    metadata: Default::default(),
    replay_item: None,
    opaque_kind: None,
    plaintext: text.to_owned(),
    display: text.to_owned(),
    signature: String::new(),
    ciphertext: String::new(),
  }))
}

fn decode_tool_use(call: &Value) -> Result<Message, Error> {
  let call_id = call
    .get("id")
    .and_then(Value::as_str)
    .ok_or_else(|| Error::Malformed("tool call is missing `id`".to_owned()))?;
  let function = call
    .get("function")
    .ok_or_else(|| Error::Malformed("tool call is missing `function`".to_owned()))?;
  let name = function
    .get("name")
    .and_then(Value::as_str)
    .ok_or_else(|| Error::Malformed("tool call function is missing `name`".to_owned()))?;
  let arguments = match function.get("arguments").and_then(Value::as_str) {
    Some(raw) if !raw.trim().is_empty() => serde_json::from_str(raw)
      .map_err(|_| Error::Malformed("tool call arguments are not valid JSON".to_owned()))?,
    _ => json!({}),
  };
  Ok(Message::ToolUse {
    metadata: Default::default(),
    call_id: call_id.to_owned(),
    name: name.to_owned(),
    arguments,
  })
}

/// Captures reasoning this wire folds into the assistant `content` as `thinking` chunks.
fn decode_thinking_chunks(
  message: &Value,
  mode: ChatCompletionApiCompatMode,
) -> Result<Option<Message>, Error> {
  if !mode.is_mistral() {
    return Ok(None);
  }
  let Some(chunks) = message.get("content").and_then(Value::as_array) else { return Ok(None) };
  let mut plaintext = String::new();
  for chunk in chunks.iter().filter(|chunk| is_thinking(chunk)) {
    let parts = chunk
      .get("thinking")
      .and_then(Value::as_array)
      .ok_or_else(|| Error::Malformed("`thinking` chunk carries no `thinking` list".to_owned()))?;
    for part in parts {
      if let Some(text) = part.get("text").and_then(Value::as_str) {
        plaintext.push_str(text);
      }
    }
  }
  if plaintext.is_empty() {
    return Ok(None);
  }
  Ok(Some(Message::Reasoning {
    metadata: Default::default(),
    replay_item: None,
    opaque_kind: None,
    plaintext: plaintext.clone(),
    display: plaintext,
    signature: String::new(),
    ciphertext: String::new(),
  }))
}

/// The refusal MiniMax reports inside a `2xx` payload, when this is one.
///
/// `base_resp.status_code` is zero for a call the service served and non-zero for one it refused,
/// with its reason in `status_msg`.
pub(crate) fn decode_refusal_error(payload: &Value) -> Option<Error> {
  let base = payload.get("base_resp")?;
  let status = base.get("status_code").and_then(Value::as_i64).filter(|status| *status != 0)?;
  let message = base
    .get("status_msg")
    .and_then(Value::as_str)
    .filter(|message| !message.is_empty())
    .unwrap_or("the service refused the call without a message");
  Some(Error::from_in_band(Some(status.to_string()), message.to_owned()))
}

/// The `type` of one content chunk.
pub(crate) fn get_chunk_type(chunk: &Value) -> Option<&str> {
  chunk.get("type").and_then(Value::as_str)
}

/// Whether a content chunk is a thought: the vendor's schema spells the type `thinking`, its prose
/// spells it `think`, so both are read.
pub(crate) fn is_thinking(chunk: &Value) -> bool {
  matches!(get_chunk_type(chunk), Some("thinking" | "think"))
}

fn decode_stop_reason(choice: &Value, mode: ChatCompletionApiCompatMode) -> StopReason {
  map_stop_reason(choice.get("finish_reason").and_then(Value::as_str), mode)
}

pub(crate) fn map_stop_reason(
  finish: Option<&str>,
  mode: ChatCompletionApiCompatMode,
) -> StopReason {
  match finish {
    Some("stop") => StopReason::Stop,
    Some("length") => StopReason::MaxOutputLengthExceeded,
    Some("tool_calls" | "function_call") => StopReason::ToolUse,
    Some("content_filter" | "sensitive") => StopReason::ContentFilter,
    Some("model_length") if mode.is_mistral() => StopReason::ContextLengthExceeded,
    // Some OpenAI-compatible servers report a full context window under this name, which is a
    // ceiling of its own rather than the output cap `length` reports.
    Some("model_context_window_exceeded") => StopReason::ContextLengthExceeded,
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
    input_tokens: field(&["prompt_tokens"]),
    cached_input_tokens: field(&["prompt_tokens_details", "cached_tokens"]),
    cache_write_input_tokens: None,
    output_tokens: field(&["completion_tokens"]),
    reasoning_tokens: field(&["completion_tokens_details", "reasoning_tokens"]),
    total_tokens: field(&["total_tokens"]),
  }
}
