use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};

pub(crate) fn decode(body: &Value) -> Result<crate::extensions::shared::images::ImageReply, Error> {
  let mut images = Vec::new();
  {
    if body["finish_reason"] == "CONTENT_FILTERED" {
      return Err(Error::Malformed("image generation was moderated".into()));
    }
    if let Some(data) = body["image"].as_str() {
      images.push(crate::extensions::shared::images::ImageSource::Base64(data.into()));
    }
  }
  if images.is_empty() || images.len() > 8 {
    return Err(Error::Malformed("no supported images or too many images".into()));
  }
  Ok(crate::extensions::shared::images::ImageReply::Complete(images))
}

pub(crate) fn submission(
  prompt: &str,
  parameters: &serde_json::Map<String, Value>,
) -> Result<Draft, Error> {
  let mut args = parameters.clone();
  args.insert("prompt".into(), json!(prompt));

  let boundary = format!("wish-{}", uuid::Uuid::new_v4());
  let mut body = String::new();
  for (key, value) in args {
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
      return Err(Error::Build("invalid image parameter name".into()));
    }
    let value = value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string());
    body.push_str(&format!(
      "--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n"
    ));
  }
  body.push_str(&format!("--{boundary}--\r\n"));
  Ok(Draft::post(
    None,
    vec![
      ("content-type".into(), format!("multipart/form-data; boundary={boundary}")),
      ("accept".into(), "application/json".into()),
    ],
    body.into_bytes(),
  ))
}
