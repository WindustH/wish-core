//! The shape of a provider error envelope, shared by the dialects.
//!
//! A non-2xx reply belongs to the protocol layer, not the transport: only the protocol knows whether
//! the envelope is `error: {type, message}` or a bare `message`. What this module owns is the part
//! that is the same everywhere: reading the body as JSON when it is JSON and as text when it is not,
//! and falling back to a rendering of the body when the protocol finds nothing worth reporting. The
//! same goes for an error a service reports inside a reply it otherwise served, which
//! [`decode_in_band`] reads, and for the provider-side reads - account state, model lists, web
//! search, a token exchange - which [`read_provider_json`] takes from status to JSON in one step.

use serde_json::Value;

use crate::protocol::attempt::Reply;
use crate::protocol::error::Error;
use crate::protocol::json_read::{read_member_text, read_string_member};

/// Longest body rendering kept in an error message.
const LIMIT: usize = 512;

/// An error a service reported inside a reply it otherwise served - a `2xx` body, or an event of a
/// stream - read from its error object.
///
/// The code is the first of `code_keys` the object has, when that member is a string, and the
/// message is its `message`, or `fallback` when it has none. Each wire files its code under a name
/// of its own, and each reader states which, so the keys and the fallback are the caller's.
pub(crate) fn decode_in_band(error: Option<&Value>, code_keys: &[&str], fallback: &str) -> Error {
  let code = error
    .and_then(|error| code_keys.iter().find_map(|key| error.get(*key)))
    .and_then(Value::as_str);
  let message =
    error.and_then(|error| error.get("message")).and_then(Value::as_str).unwrap_or(fallback);
  Error::from_in_band(code.map(str::to_owned), message.to_owned())
}

/// Builds the error for a non-`2xx` reply.
///
/// `code` and `message` are whatever the protocol managed to dig out; when there is no message the
/// body itself is rendered instead, so a gateway's HTML error page still says something. The status
/// is always prefixed: it is the one fact that survives every envelope shape.
pub fn from_envelope(
  status: u16,
  code: Option<String>,
  message: Option<String>,
  body: &Value,
) -> Error {
  let message = message.unwrap_or_else(|| excerpt_body(&body.to_string()));
  Error::from_http(status, code, format!("HTTP {status}: {message}"))
}

/// The error for a non-`2xx` body: `decode_envelope`'s reading of it when it is JSON, and the
/// status with an excerpt of the body when it is not - a proxy's HTML page is no envelope, and
/// the status still has to say what happened.
pub fn decode_error_reply(
  status: u16,
  body: &[u8],
  decode_envelope: impl FnOnce(u16, &Value) -> Error,
) -> Error {
  match serde_json::from_slice::<Value>(body) {
    Ok(value) => decode_envelope(status, &value),
    Err(_) => Error::from_http(status, None, format!("HTTP {status}: {}", decode_body_text(body))),
  }
}

/// The text of a body that never parsed as JSON at all.
pub fn decode_body_text(bytes: &[u8]) -> String {
  excerpt_body(&String::from_utf8_lossy(bytes))
}

/// Anthropic messages: `error: {type, message}`.
pub fn decode_anthropic_messages(status: u16, body: &Value) -> Error {
  let error = body.get("error");
  from_envelope(
    status,
    read_string_member(error, "type"),
    read_string_member(error, "message"),
    body,
  )
}

/// OpenAI's envelope, which both chat completions and responses answer with: `error: {message,
/// type, code}`, where `code` is often null.
pub fn decode_openai_envelope(status: u16, body: &Value) -> Error {
  // This is the wire everything else imitates, and the imitations drift: some drop the `error`
  // wrapper and put the report at the top of the body, some make `error` a bare string, and some
  // file it under FastAPI's `detail`. Whichever the body puts the report in, that is the envelope.
  let error = body.get("error").or_else(|| body.get("detail")).unwrap_or(body);
  let code =
    read_member_text(Some(error), "code").or_else(|| read_member_text(Some(error), "type"));
  let message = read_string_member(Some(error), "message")
    .or_else(|| error.as_str().filter(|text| !text.is_empty()).map(str::to_owned));
  from_envelope(status, code, message, body)
}

/// Gemini's envelope, which `generateContent` and interactions both answer with:
/// `error: {code, message, status}`, where `code` repeats the HTTP status as a number and `status`
/// is the canonical string (`RESOURCE_EXHAUSTED` and friends).
pub fn decode_google_envelope(status: u16, body: &Value) -> Error {
  let error = body.get("error");
  from_envelope(
    status,
    read_string_member(error, "status"),
    read_string_member(error, "message"),
    body,
  )
}

/// Bedrock Converse: a bare `{message}`, sometimes with `__type` naming the exception.
pub fn decode_bedrock_converse(status: u16, body: &Value) -> Error {
  // The runtime spells it `message`; the gateway in front of it spells it `Message`.
  let message =
    read_string_member(Some(body), "message").or_else(|| read_string_member(Some(body), "Message"));
  from_envelope(status, read_string_member(Some(body), "__type"), message, body)
}

/// Mistral: `{code, message}` with an optional `type`, the same on both of its wires.
pub fn decode_mistral_conversations(status: u16, body: &Value) -> Error {
  // Both of its wires answer with `{code, message}`, and the gateway in front of them answers with
  // FastAPI's `{detail}`.
  let code = read_member_text(Some(body), "code").or_else(|| read_member_text(Some(body), "type"));
  let message =
    read_string_member(Some(body), "message").or_else(|| read_string_member(Some(body), "detail"));
  from_envelope(status, code, message, body)
}

/// A body cut down to what an error message keeps: trimmed, at most [`LIMIT`] characters, and
/// named as empty rather than left blank.
fn excerpt_body(text: &str) -> String {
  let trimmed = text.trim();
  if trimmed.is_empty() {
    return "(empty body)".to_owned();
  }
  trimmed.chars().take(LIMIT).collect()
}

/// Maps a non-`2xx` body from a provider-side read: the members these services put a code and a
/// message in, or the body itself when there is neither.
///
/// The shapes are a bare `{code, message}` with the code a string or a number, an OpenAI-style
/// `{error: {message, code}}`, Aliyun's `{Code, Message}`, MiniMax's nested
/// `{base_resp: {status_code, status_msg}}`, a bare string under `error`, and FastAPI's `{detail}`;
/// the status is prefixed either way, because the status is the one fact every shape shares.
pub fn decode_provider_envelope(status: u16, body: &[u8]) -> Error {
  decode_error_reply(status, body, decode_provider_value)
}

fn decode_provider_value(status: u16, value: &Value) -> Error {
  // One service answers with a bare string under `error`, and that string is the whole report.
  let base = value
    .get("base_resp")
    .or_else(|| value.get("error"))
    .or_else(|| value.get("detail"))
    .unwrap_or(value);
  let code = read_member_text(Some(base), "code")
    .or_else(|| read_member_text(Some(base), "Code"))
    .or_else(|| read_member_text(Some(base), "type"))
    .or_else(|| read_member_text(Some(base), "status_code"));
  let message = read_string_member(Some(base), "message")
    .or_else(|| read_string_member(Some(base), "Message"))
    .or_else(|| read_string_member(Some(base), "status_msg"))
    .or_else(|| read_string_member(Some(base), "msg"))
    // `{"detail": {"error": "..."}}` (Tavily) names its message this way.
    .or_else(|| read_string_member(Some(base), "error"))
    // Brave's `{"error": {"code", "detail"}}`.
    .or_else(|| read_string_member(Some(base), "detail"))
    .or_else(|| base.as_str().filter(|text| !text.is_empty()).map(str::to_owned));
  from_envelope(status, code, message, value)
}

/// A successful body as JSON, or the malformed reply `label` names the body by.
pub fn decode_json_body(label: &str, bytes: &[u8]) -> Result<Value, Error> {
  serde_json::from_slice(bytes).map_err(|error| {
    Error::Malformed(format!(
      "{label} response body is not JSON ({error}): {}",
      decode_body_text(bytes)
    ))
  })
}

/// The JSON body of a provider-side read: the provider envelope's failure for a non-`2xx` reply,
/// with its `retry-after`, and the body as JSON otherwise, named by `label` when it is not.
pub(crate) fn read_provider_json(reply: Reply, label: &str) -> Result<Value, Error> {
  let reply = reply.require_success(decode_provider_envelope)?;
  decode_json_body(label, &reply.body)
}
