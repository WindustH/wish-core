//! Grok Imagine JSON generation/editing; its image references differ from GPT Image.
use crate::extensions::shared::images::ReferenceInput;
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Map, Value, json};
fn invalid(message: &str) -> Error {
  Error::Build(message.into())
}
pub(crate) fn reference_limit(model: &str) -> usize {
  if model == "grok-imagine-image-2.0" { 5 } else { 0 }
}
pub(crate) fn request_options(model: &str) -> &'static [&'static str] {
  if reference_limit(model) > 0 { &["aspect_ratio", "image_size", "quality", "n"] } else { &[] }
}
pub(crate) fn validate(model: &str, parameters: &Map<String, Value>) -> Result<(), Error> {
  for key in parameters.keys() {
    if !["aspect_ratio", "resolution", "quality", "n", "response_format"].contains(&key.as_str()) {
      return Err(invalid("unsupported xAI image parameter"));
    }
  }
  if parameters.get("n").is_some_and(|v| !v.as_u64().is_some_and(|n| (1..=10).contains(&n))) {
    return Err(invalid("xAI image count must be an integer from 1 to 10"));
  }
  if parameters.get("aspect_ratio").is_some_and(|v| {
    !v.as_str().is_some_and(|v| {
      [
        "auto", "1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3", "2:1", "1:2", "19.5:9",
        "9:19.5", "20:9", "9:20", "21:9", "5:2",
      ]
      .contains(&v)
    })
  }) {
    return Err(invalid("unsupported xAI aspect_ratio"));
  }
  if parameters.get("resolution").is_some_and(|v| !matches!(v.as_str(), Some("1k" | "2k"))) {
    return Err(invalid("xAI resolution must be 1k or 2k"));
  }
  if let Some(quality) = parameters.get("quality")
    && (model != "grok-imagine-image-2.0"
      || !matches!(quality.as_str(), Some("auto" | "low" | "medium")))
  {
    return Err(invalid(
      "xAI image quality requires grok-imagine-image-2.0 and auto, low or medium",
    ));
  }
  if parameters
    .get("response_format")
    .is_some_and(|v| !matches!(v.as_str(), Some("url" | "b64_json")))
  {
    return Err(invalid("xAI response_format must be url or b64_json"));
  }
  Ok(())
}
pub(crate) fn submission(
  model: &str,
  prompt: &str,
  parameters: &Map<String, Value>,
  images: &[ReferenceInput],
) -> Result<Draft, Error> {
  validate(model, parameters)?;
  if images.len() > reference_limit(model) {
    return Err(invalid("xAI reference model/count is unsupported"));
  }
  let mut body = parameters.clone();
  body.insert("model".into(), json!(model));
  body.insert("prompt".into(), json!(prompt));
  let images: Vec<_> = images
    .iter()
    .map(|input| {
      let url = match input {
        ReferenceInput::Inline(image) => format!("data:{};base64,{}", image.mime, image.data),
        ReferenceInput::Url(url) => url.clone(),
      };
      json!({"type":"image_url", "url":url})
    })
    .collect();
  let editing = !images.is_empty();
  if images.len() == 1 {
    body.insert("image".into(), images[0].clone());
  } else if editing {
    body.insert("images".into(), json!(images));
  }
  let mut draft = Draft::post_json(&Value::Object(body))?;
  if editing {
    draft.path = Some("/images/edits".into());
  }
  Ok(draft)
}

/// Merge this supplier's public per-call controls into its own wire configuration.
pub(crate) fn merge_options(
  mut defaults: serde_json::Map<String, serde_json::Value>,
  options: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, crate::protocol::error::Error> {
  for (key, value) in options {
    if key == "image_size" {
      let resolution = match value.as_str() {
        Some("1K") => "1k",
        Some("2K") => "2k",
        _ => return Err(Error::Build("xAI image_size must be 1K or 2K".into())),
      };
      defaults.insert("resolution".into(), json!(resolution));
    } else {
      defaults.insert(key.clone(), value.clone());
    }
  }

  Ok(defaults)
}

pub(crate) fn decode(body: &Value) -> Result<crate::extensions::shared::images::ImageReply, Error> {
  use crate::extensions::shared::images::{ImageReply, ImageSource};
  let images = body["data"]
    .as_array()
    .into_iter()
    .flatten()
    .filter_map(|item| {
      if let Some(data) =
        item.get("b64_json").or_else(|| item.get("base64")).and_then(Value::as_str)
      {
        Some(ImageSource::Base64(data.into()))
      } else {
        item
          .get("url")
          .and_then(Value::as_str)
          .or_else(|| item.as_str())
          .map(|url| ImageSource::Url(url.into()))
      }
    })
    .collect::<Vec<_>>();
  if images.is_empty() || images.len() > 10 {
    return Err(Error::Malformed(
      "xAI response contains no supported images or exceeds 10 images".into(),
    ));
  }
  Ok(ImageReply::Complete(images))
}

pub fn capabilities(model: &str) -> crate::extensions::shared::images::ImageCapabilities {
  crate::extensions::shared::images::ImageCapabilities {
    reference_limit: reference_limit(model),
    local_references: true,
    remote_references: true,
    image_required: false,
    mask: false,
    markup: false,
    options: request_options(model),
  }
}
