use crate::protocol::error::Error;
use serde_json::Value;

pub(crate) fn decode(
  body: &Value,
  current_url: &str,
  result_url: Option<&str>,
) -> Result<crate::extensions::shared::images::ImageReply, Error> {
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
    if body.get("error").is_some_and(|v| !v.is_null()) {
      return Err(Error::Malformed("image generation failed".into()));
    }
    for image in body["images"].as_array().into_iter().flatten() {
      add(image);
    }
    if images.is_empty() {
      let result = body["response_url"].as_str().or(result_url);
      let url = if body["status"] == "COMPLETED" {
        result
      } else {
        body["status_url"].as_str().or_else(|| {
          matches!(body["status"].as_str(), Some("IN_QUEUE" | "IN_PROGRESS")).then_some(current_url)
        })
      };
      if let Some(url) = url {
        return Ok(crate::extensions::shared::images::ImageReply::Pending {
          url: url.into(),
          result_url: result.map(str::to_owned),
        });
      }
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
  let _ = model;
  crate::protocol::endpoint::Draft::post_json(&serde_json::Value::Object(args))
}
