//! Bedrock Converse response wire.
//!
//! Conversions:
//! - `output.message.content[]` decodes in order: `text` accumulates into an `Assistant` message,
//!   every `toolUse` becomes its own `ToolUse` message, and `reasoningContent` becomes `Reasoning`
//!   (`reasoningText` into `plaintext` / `display` with its `signature`, `redactedContent` into
//!   `ciphertext`). Assistant text is flushed before each reasoning block so `Reasoning` leads its
//!   assistant turn.
//! - `stopReason` maps `end_turn` / `stop_sequence` to `Stop`, `tool_use` to `ToolUse`, `max_tokens`
//!   to `MaxOutputLengthExceeded`, `guardrail_intervened` / `content_filtered` to `ContentFilter`,
//!   and `malformed_tool_use` to `MalformedToolUse`; `malformed_model_output` is `Unknown`, like
//!   anything else the wire adds later.
//! - Usage carries the cache counters, so `cached_input_tokens` and `cache_write_input_tokens` are
//!   populated here, unlike on the Google wires.
//!
//! Trade-offs:
//! - The wire uses two shapes for reasoning data: the nested `reasoningText` object of responses and
//!   the flat `reasoningContent.text` of stream deltas. Both decode, the nested one is sent.
//! - The AWS error envelope (`message` / `__type`) maps to `Error::Upstream` with `__type` as the
//!   code.

use crate::protocol::error::Error;
use crate::protocol::ReasoningOpaqueKind;
use crate::protocol::{ContentBlock, Message, Response, StopReason, Usage};
use serde_json::{Value, json};

pub fn decode(body: &Value) -> Result<Response, Error> {
  // An in-band failure: this wire reports one with a `message` and a `__type` (or `code`) beside it.
  if let Some(message) = body.get("message").and_then(Value::as_str) {
    let code = body.get("__type").or_else(|| body.get("code")).and_then(Value::as_str);
    return Err(Error::from_in_band(code.map(str::to_owned), message.to_owned()));
  }

  let mut messages: Vec<Message> = Vec::new();
  if let Some(content) = body.pointer("/output/message/content").and_then(Value::as_array) {
    let mut blocks: Vec<ContentBlock> = Vec::new();
    let flush = |messages: &mut Vec<Message>, blocks: &mut Vec<ContentBlock>| {
      if blocks.is_empty() {
        return;
      }
      messages
        .push(Message::Assistant { metadata: Default::default(), content: std::mem::take(blocks) });
    };
    for block in content {
      if let Some(reasoning) = decode_reasoning(block) {
        flush(&mut messages, &mut blocks);
        messages.push(reasoning);
      } else if let Some(tool_use) = decode_tool_use(block)? {
        flush(&mut messages, &mut blocks);
        messages.push(tool_use);
      } else if let Some(text) = block.get("text").and_then(Value::as_str) {
        blocks.push(ContentBlock::Text { text: text.to_owned() });
      }
    }
    flush(&mut messages, &mut blocks);
  }
  Ok(Response {
    messages,
    stop_reason: decode_stop_reason(body),
    usage: decode_usage(body),
    account_state: None,
  })
}

fn decode_reasoning(block: &Value) -> Option<Message> {
  let reasoning = block.get("reasoningContent")?;
  if let Some(ciphertext) = reasoning.get("redactedContent").and_then(Value::as_str) {
    return Some(Message::Reasoning {
      metadata: Default::default(),
      replay_item: None,
      opaque_kind: Some(ReasoningOpaqueKind::BedrockRedacted),
      plaintext: String::new(),
      display: String::new(),
      signature: String::new(),
      ciphertext: ciphertext.to_owned(),
    });
  }
  let reasoning_text = reasoning.get("reasoningText");
  let text = reasoning_text
    .and_then(|reasoning_text| reasoning_text.get("text"))
    .or_else(|| reasoning.get("text"))
    .and_then(Value::as_str)
    .unwrap_or("");
  let signature = reasoning_text
    .and_then(|reasoning_text| reasoning_text.get("signature"))
    .or_else(|| reasoning.get("signature"))
    .and_then(Value::as_str)
    .unwrap_or("");
  Some(Message::Reasoning {
    metadata: Default::default(),
    replay_item: None,
    opaque_kind: (!signature.is_empty()).then_some(ReasoningOpaqueKind::BedrockSignature),
    plaintext: text.to_owned(),
    display: text.to_owned(),
    signature: signature.to_owned(),
    ciphertext: String::new(),
  })
}

fn decode_tool_use(block: &Value) -> Result<Option<Message>, Error> {
  let Some(tool_use) = block.get("toolUse") else {
    return Ok(None);
  };
  let name = tool_use
    .get("name")
    .and_then(Value::as_str)
    .ok_or_else(|| Error::Malformed("toolUse block is missing `name`".to_owned()))?;
  let call_id = tool_use.get("toolUseId").and_then(Value::as_str).unwrap_or("");
  let arguments = tool_use.get("input").cloned().unwrap_or_else(|| json!({}));
  Ok(Some(Message::ToolUse {
    metadata: Default::default(),
    call_id: call_id.to_owned(),
    name: name.to_owned(),
    arguments,
  }))
}

fn decode_stop_reason(body: &Value) -> StopReason {
  map_stop_reason(body.get("stopReason").and_then(Value::as_str))
}

pub(crate) fn map_stop_reason(reason: Option<&str>) -> StopReason {
  match reason {
    Some("end_turn" | "stop_sequence") => StopReason::Stop,
    Some("tool_use") => StopReason::ToolUse,
    Some("max_tokens") => StopReason::MaxOutputLengthExceeded,
    Some("guardrail_intervened" | "content_filtered") => StopReason::ContentFilter,
    Some("malformed_tool_use") => StopReason::MalformedToolUse,
    _ => StopReason::Unknown,
  }
}

fn decode_usage(body: &Value) -> Usage {
  parse_usage(body.get("usage"))
}

pub(crate) fn parse_usage(usage: Option<&Value>) -> Usage {
  let field = |name: &str| usage.and_then(|usage| usage.get(name)).and_then(Value::as_u64);
  Usage {
    input_tokens: field("inputTokens"),
    cached_input_tokens: field("cacheReadInputTokens"),
    cache_write_input_tokens: field("cacheWriteInputTokens"),
    output_tokens: field("outputTokens"),
    reasoning_tokens: None,
    total_tokens: field("totalTokens"),
  }
}
