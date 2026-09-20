//! The shape of a provider error envelope, shared by the dialects.
//!
//! A non-2xx reply belongs to the protocol layer, not the transport: only the protocol knows whether
//! the envelope is `error: {type, message}` or a bare `message`. What this module owns is the part
//! that is the same everywhere: reading one member as text, and falling back to a rendering of the
//! body when the protocol finds nothing worth reporting.

use serde_json::Value;

use crate::protocol::error::Error;

/// Longest body rendering kept in an error message.
const LIMIT: usize = 512;

/// The text of one string member of an object, when it is there, is a string and is not empty.
pub fn read_string_member(object: Option<&Value>, key: &str) -> Option<String> {
  let text = object?.get(key)?.as_str()?;
  (!text.is_empty()).then(|| text.to_owned())
}

/// One member read as text when it carries something: a string, or a number.
///
/// A code arrives as a string from most services and as a number from several others - the wire
/// envelope decides that, not us - and either spelling is the code.
pub fn read_member_text(object: Option<&Value>, key: &str) -> Option<String> {
  match object?.get(key)? {
    Value::String(text) => (!text.is_empty()).then(|| text.to_owned()),
    Value::Number(number) => Some(number.to_string()),
    _ => None,
  }
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
  let message = message.unwrap_or_else(|| truncate(&body.to_string()));
  Error::from_http(status, code, format!("HTTP {status}: {message}"))
}

/// The text of a body that never parsed as JSON at all.
pub fn decode_body_text(bytes: &[u8]) -> String {
  truncate(&String::from_utf8_lossy(bytes))
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

/// OpenAI chat completions: `error: {message, type, code}`, where `code` is often null.
pub fn decode_openai_chat(status: u16, body: &Value) -> Error {
  decode_openai_envelope(status, body)
}

/// OpenAI responses: the same envelope as chat completions.
pub fn decode_openai_responses(status: u16, body: &Value) -> Error {
  decode_openai_envelope(status, body)
}

/// Gemini `generateContent`: `error: {code, message, status}`, where `code` repeats the HTTP status
/// as a number and `status` is the canonical string (`RESOURCE_EXHAUSTED` and friends).
pub fn decode_google_generate_content(status: u16, body: &Value) -> Error {
  let error = body.get("error");
  from_envelope(
    status,
    read_string_member(error, "status"),
    read_string_member(error, "message"),
    body,
  )
}

/// Gemini interactions: the same `error: {code, message, status}` envelope.
pub fn decode_google_interactions(status: u16, body: &Value) -> Error {
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

fn decode_openai_envelope(status: u16, body: &Value) -> Error {
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

fn truncate(text: &str) -> String {
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
  let Ok(value) = serde_json::from_slice::<Value>(body) else {
    return Error::from_http(status, None, format!("HTTP {status}: {}", decode_body_text(body)));
  };
  // One service answers with a bare string under `error`, and that string is the whole report.
  let base = value
    .get("base_resp")
    .or_else(|| value.get("error"))
    .or_else(|| value.get("detail"))
    .unwrap_or(&value);
  let code = read_member_text(Some(base), "code")
    .or_else(|| read_member_text(Some(base), "Code"))
    .or_else(|| read_member_text(Some(base), "type"))
    .or_else(|| read_member_text(Some(base), "status_code"));
  let message = read_string_member(Some(base), "message")
    .or_else(|| read_string_member(Some(base), "Message"))
    .or_else(|| read_string_member(Some(base), "status_msg"))
    .or_else(|| read_string_member(Some(base), "msg"))
    .or_else(|| base.as_str().filter(|text| !text.is_empty()).map(str::to_owned));
  from_envelope(status, code, message, &value)
}

/// A successful body as JSON, or the malformed reply a protocol would report for it.
pub fn decode_json_body(protocol: &str, bytes: &[u8]) -> Result<Value, Error> {
  serde_json::from_slice(bytes).map_err(|error| {
    Error::Malformed(format!(
      "{protocol} response body is not JSON ({error}): {}",
      decode_body_text(bytes)
    ))
  })
}
