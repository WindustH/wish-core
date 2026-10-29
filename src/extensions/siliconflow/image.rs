use crate::protocol::error::Error;
use serde_json::Value;

pub(crate) fn decode(body: &Value) -> Result<crate::extensions::shared::images::ImageReply, Error> {
  let mut images = Vec::new();
  let mut add = |item: &Value| {
    if let Some(data) = item.get("b64_json").or_else(|| item.get("base64")).and_then(Value::as_str)
    {
      images.push(crate::extensions::shared::images::ImageSource::Base64(data.into()));
    } else if let Some(url) = item.get("url").and_then(Value::as_str).or_else(|| item.as_str()) {
      images.push(crate::extensions::shared::images::ImageSource::Url(url.into()));
    }
  };
  {
    for image in body["images"].as_array().into_iter().flatten() {
      add(image);
    }
  }
  if images.is_empty() || images.len() > 8 {
    return Err(Error::Malformed("no supported images or too many images".into()));
  }
  Ok(crate::extensions::shared::images::ImageReply::Complete(images))
}

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
