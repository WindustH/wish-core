//! Subscription extension calls use the same project and OAuth bearer as chat.
use crate::protocol::{
  endpoint::{AuthScheme, Credentials, Draft, Source, antigravity::API_HEADERS},
  error::Error,
};
use serde_json::{Value, json};

pub fn source(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://daily-cloudcode-pa.googleapis.com"),
    path: Some("/v1internal:generateContent"),
    auth: AuthScheme::Bearer(None),
    headers: API_HEADERS,
  }
}

pub fn draft(
  model: &str,
  request: Value,
  kind: &str,
  credentials: &Credentials,
) -> Result<Draft, Error> {
  let mut body = crate::protocol::model_use::request::google_antigravity::wrap(
    request,
    model,
    credentials.project.as_deref(),
    &uuid::Uuid::new_v4().to_string(),
  )?;
  body["requestType"] = json!(kind);
  Draft::post_json(&body)
}

pub fn response(body: &Value) -> Result<&Value, Error> {
  body
    .get("response")
    .filter(|v| v.is_object())
    .ok_or_else(|| Error::Malformed("Antigravity extension response has no response object".into()))
}
