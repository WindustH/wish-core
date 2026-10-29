//! Supplier image wire format.

pub(crate) fn generation(
  model: &str,
  prompt: &str,
  parameters: &serde_json::Map<String, serde_json::Value>,
) -> Result<crate::protocol::endpoint::Draft, crate::protocol::error::Error> {
  let mut args = parameters.clone();
  args.insert("prompt".into(), serde_json::json!(prompt));
  args.insert("model".into(), serde_json::json!(model));
  crate::protocol::endpoint::Draft::post_json(&serde_json::Value::Object(args))
}

pub(crate) fn decode(
  body: &serde_json::Value,
) -> Result<crate::extensions::shared::images::ImageReply, crate::protocol::error::Error> {
  use crate::extensions::shared::images::{ImageReply, ImageSource};
  let mut images = Vec::new();
  for item in body["data"].as_array().into_iter().flatten() {
    if let Some(data) =
      item.get("b64_json").or_else(|| item.get("base64")).and_then(serde_json::Value::as_str)
    {
      images.push(ImageSource::Base64(data.into()));
    } else if let Some(url) =
      item.get("url").and_then(serde_json::Value::as_str).or_else(|| item.as_str())
    {
      images.push(ImageSource::Url(url.into()));
    }
  }
  if images.is_empty() || images.len() > 8 {
    return Err(crate::protocol::error::Error::Malformed(
      "image response contains no supported images or exceeds 8 images".into(),
    ));
  }
  Ok(ImageReply::Complete(images))
}
