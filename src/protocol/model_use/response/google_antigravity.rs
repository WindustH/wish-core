//! Cloud Code's response envelope around a Gemini response, including in-band errors.
use crate::protocol::{Response, error::Error};
use serde_json::Value;

pub(crate) fn unwrap(body: &Value) -> Result<&Value, Error> {
  if body.get("error").is_some_and(Value::is_object) {
    return Err(super::google_generate_content::decode_in_band_error(body));
  }
  body
    .get("response")
    .filter(|value| value.is_object())
    .ok_or_else(|| Error::Malformed("Antigravity reply has no response object".to_owned()))
}

pub fn decode(body: &Value) -> Result<Response, Error> {
  super::google_generate_content::decode(unwrap(body)?)
}
