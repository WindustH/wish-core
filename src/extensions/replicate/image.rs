use crate::protocol::error::Error;
use serde_json::Value;

pub(crate) fn decode(
  body: &Value,
  current_url: &str,
  _result_url: Option<&str>,
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
  match body["status"].as_str() {
    Some("failed" | "canceled") => {
      return Err(Error::Malformed("image prediction failed or was canceled".into()));
    }
    Some("succeeded") => {
      if let Some(output) = body["output"].as_array() {
        for image in output {
          add(image);
        }
      } else {
        add(&body["output"]);
      }
    }
    Some("starting" | "processing") => {
      return Ok(crate::extensions::shared::images::ImageReply::Pending {
        url: body["urls"]["get"].as_str().unwrap_or(current_url).into(),
        result_url: None,
      });
    }
    _ => {}
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
  crate::protocol::endpoint::Draft::post_json(&serde_json::json!({"input":args}))
}
