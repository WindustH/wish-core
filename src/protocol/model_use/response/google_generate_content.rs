//! Google `generateContent` response wire.
//!
//! Conversions:
//! - The reply is read from `candidates[0].content.parts`, in order.
//! - A `thought: true` part becomes `Reasoning` with its text in `plaintext` / `display` and the
//!   `thoughtSignature` in `signature`. A signature sitting on a `functionCall` or text part becomes
//!   a signature-only `Reasoning` emitted just before it, so the request side can attach it back to
//!   the part it came from.
//! - `functionCall` becomes `ToolUse` (`name` required, `id` optional), text parts accumulate into
//!   `Assistant`, and `inlineData` becomes `Image`.
//! - `finishReason` maps `STOP` to `Stop` unless a call was made (`ToolUse`), `MAX_TOKENS` to
//!   `MaxOutputLengthExceeded`, the safety family (`SAFETY`, `RECITATION`, `BLOCKLIST`,
//!   `PROHIBITED_CONTENT`, `SPII`, `IMAGE_SAFETY`) to `ContentFilter` and
//!   `MALFORMED_FUNCTION_CALL` to `MalformedToolUse`.
//!
//! Trade-offs:
//! - Candidates past the first are dropped: a reply carrying several keeps only `candidates[0]`.
//! - `promptFeedback.blockReason` (a prompt blocked before generation) is not modeled yet, and the
//!   wire's error envelope maps to `Error::Upstream` with `error.status` as the code.

use crate::protocol::error::Error;
use crate::protocol::{ContentBlock, Message, Response, StopReason, Usage};
use serde_json::{Value, json};

pub fn decode(body: &Value) -> Result<Response, Error> {
  if body.get("error").is_some() {
    return Err(decode_upstream_error(body));
  }

  let mut messages: Vec<Message> = Vec::new();
  let mut content: Vec<ContentBlock> = Vec::new();
  let flush = |messages: &mut Vec<Message>, content: &mut Vec<ContentBlock>| {
    if content.is_empty() {
      return;
    }
    messages
      .push(Message::Assistant { metadata: Default::default(), content: std::mem::take(content) });
  };
  // A signature that arrives on its own (a bare `thoughtSignature` part, or a `thought` part
  // without text) belongs to the reasoning it concludes: fill the previous reasoning message
  // when it carries no signature yet, otherwise become its own signature-only reasoning so the
  // replay rule (§4.2) never drops it.
  let land_signature =
    |messages: &mut Vec<Message>, content: &mut Vec<ContentBlock>, proof: &str| {
      if proof.is_empty() {
        return;
      }
      match messages.last_mut() {
        Some(Message::Reasoning { signature, .. }) if signature.is_empty() => {
          *signature = proof.to_owned();
        }
        _ => {
          flush(messages, content);
          messages.push(Message::Reasoning {
            metadata: Default::default(),
            replay_item: None,
            plaintext: String::new(),
            display: String::new(),
            signature: proof.to_owned(),
            ciphertext: String::new(),
          });
        }
      }
    };
  if let Some(parts) = body.pointer("/candidates/0/content/parts").and_then(Value::as_array) {
    for part in parts {
      let signature = part.get("thoughtSignature").and_then(Value::as_str).unwrap_or("");
      if part.get("thought") == Some(&Value::Bool(true)) {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
          flush(&mut messages, &mut content);
          messages.push(Message::Reasoning {
            metadata: Default::default(),
            replay_item: None,
            plaintext: text.to_owned(),
            display: text.to_owned(),
            signature: signature.to_owned(),
            ciphertext: String::new(),
          });
          continue;
        }
        land_signature(&mut messages, &mut content, signature);
        continue;
      }
      if let Some(function_call) = part.get("functionCall") {
        flush(&mut messages, &mut content);
        if !signature.is_empty() {
          messages.push(Message::Reasoning {
            metadata: Default::default(),
            replay_item: None,
            plaintext: String::new(),
            display: String::new(),
            signature: signature.to_owned(),
            ciphertext: String::new(),
          });
        }
        messages.push(decode_function_call(function_call)?);
        continue;
      }
      if let Some(text) = part.get("text").and_then(Value::as_str) {
        if !signature.is_empty() {
          flush(&mut messages, &mut content);
          messages.push(Message::Reasoning {
            metadata: Default::default(),
            replay_item: None,
            plaintext: String::new(),
            display: String::new(),
            signature: signature.to_owned(),
            ciphertext: String::new(),
          });
        }
        content.push(ContentBlock::Text { text: text.to_owned() });
        continue;
      }
      if let Some(inline_data) = part.get("inlineData") {
        content.push(ContentBlock::Image {
          mime_type: inline_data.get("mimeType").and_then(Value::as_str).unwrap_or("").to_owned(),
          data_base64: inline_data.get("data").and_then(Value::as_str).unwrap_or("").to_owned(),
        });
        continue;
      }
      // A part that carries nothing but a signature still lands; everything else is tolerated.
      land_signature(&mut messages, &mut content, signature);
    }
  }
  flush(&mut messages, &mut content);

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
    .unwrap_or("upstream reported an error without a message");
  let code = error.and_then(|error| error.get("status")).and_then(Value::as_str);
  Error::from_in_band(code.map(str::to_owned), message.to_owned())
}

fn decode_function_call(function_call: &Value) -> Result<Message, Error> {
  let name = function_call
    .get("name")
    .and_then(Value::as_str)
    .ok_or_else(|| Error::Malformed("functionCall part is missing `name`".to_owned()))?;
  let call_id = function_call.get("id").and_then(Value::as_str).unwrap_or("");
  let arguments = function_call.get("args").cloned().unwrap_or_else(|| json!({}));
  Ok(Message::ToolUse {
    metadata: Default::default(),
    call_id: call_id.to_owned(),
    name: name.to_owned(),
    arguments,
  })
}

fn decode_stop_reason(body: &Value, has_tool_uses: bool) -> StopReason {
  map_stop_reason(body.pointer("/candidates/0/finishReason").and_then(Value::as_str), has_tool_uses)
}

pub(crate) fn map_stop_reason(finish: Option<&str>, has_tool_uses: bool) -> StopReason {
  match finish {
    Some("STOP") if has_tool_uses => StopReason::ToolUse,
    Some("STOP") => StopReason::Stop,
    None if has_tool_uses => StopReason::ToolUse,
    Some("MAX_TOKENS") => StopReason::MaxOutputLengthExceeded,
    Some("MALFORMED_FUNCTION_CALL") => StopReason::MalformedToolUse,
    Some(
      "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY",
    ) => StopReason::ContentFilter,
    _ => StopReason::Unknown,
  }
}

fn decode_usage(body: &Value) -> Usage {
  parse_usage(body)
}

/// Maps the top-level `usageMetadata`. The wire reports thinking tokens separately in
/// `thoughtsTokenCount` while billing them as output; `output_tokens` folds them in so the
/// number is what the model was charged, matching the dialects whose completion count already
/// includes reasoning.
pub(crate) fn parse_usage(body: &Value) -> Usage {
  let usage = body.get("usageMetadata");
  let field =
    |key: &str| -> Option<u64> { usage.and_then(|usage| usage.get(key)).and_then(Value::as_u64) };
  let candidates = field("candidatesTokenCount");
  let thoughts = field("thoughtsTokenCount");
  Usage {
    input_tokens: field("promptTokenCount"),
    cached_input_tokens: field("cachedContentTokenCount"),
    cache_write_input_tokens: None,
    output_tokens: match (candidates, thoughts) {
      (Some(candidates), Some(thoughts)) => Some(candidates + thoughts),
      (candidates, thoughts) => candidates.or(thoughts),
    },
    reasoning_tokens: thoughts,
    total_tokens: field("totalTokenCount"),
  }
}
