//! Native generateContent image controls, distinct from the Interactions wire format.
use crate::protocol::error::Error;
use serde_json::{Map, Value};
fn modern(model: &str) -> bool {
  matches!(model, "gemini-3.1-flash-image-preview" | "gemini-3.1-flash-image")
}
fn pro(model: &str) -> bool {
  matches!(model, "gemini-3-pro-image-preview" | "gemini-3-pro-image")
}
pub fn request_options(model: &str) -> &'static [&'static str] {
  if modern(model) || pro(model) {
    &["aspect_ratio", "image_size"]
  } else if model == "gemini-2.5-flash-image" {
    &["aspect_ratio"]
  } else {
    &[]
  }
}
pub fn validate(model: &str, parameters: &Map<String, Value>) -> Result<(), Error> {
  let Some(value) = parameters.get("imageConfig") else {
    return Ok(());
  };
  let config =
    value.as_object().ok_or_else(|| Error::Build("Gemini imageConfig must be an object".into()))?;
  if config.keys().any(|k| !["aspectRatio", "imageSize"].contains(&k.as_str())) {
    return Err(Error::Build("unsupported Gemini imageConfig field".into()));
  }
  if let Some(ratio) = config.get("aspectRatio") {
    let ratio =
      ratio.as_str().ok_or_else(|| Error::Build("Gemini aspectRatio must be a string".into()))?;
    if !["1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"].contains(&ratio)
      && !(modern(model) && ["1:4", "4:1", "1:8", "8:1"].contains(&ratio))
    {
      return Err(Error::Build("unsupported Gemini aspect ratio for this model".into()));
    }
  }
  if let Some(size) = config.get("imageSize") {
    let valid = size.as_str().is_some_and(|s| {
      ((modern(model) || pro(model)) && ["1K", "2K", "4K"].contains(&s))
        || (modern(model) && s == "512")
    });
    if !valid {
      return Err(Error::Build("unsupported Gemini image size for this model".into()));
    }
  }
  Ok(())
}

pub(crate) fn decode(body: &Value) -> Result<crate::extensions::shared::images::ImageReply, Error> {
  let mut images = Vec::new();
  {
    for part in body["candidates"]
      .as_array()
      .into_iter()
      .flatten()
      .flat_map(|v| v["content"]["parts"].as_array().into_iter().flatten())
    {
      if part["thought"] == true {
        continue;
      }
      if let Some(data) =
        part["inlineData"]["data"].as_str().or_else(|| part["inline_data"]["data"].as_str())
      {
        images.push(crate::extensions::shared::images::ImageSource::Base64(data.into()));
      }
    }
  }
  if images.is_empty() || images.len() > 8 {
    return Err(Error::Malformed("no supported images or too many images".into()));
  }
  Ok(crate::extensions::shared::images::ImageReply::Complete(images))
}

/// Merge this supplier's public per-call controls into its own wire configuration.
pub(crate) fn merge_options(
  mut defaults: serde_json::Map<String, serde_json::Value>,
  options: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, crate::protocol::error::Error> {
  if options.is_empty() {
    return Ok(defaults);
  }
  let config = defaults
    .entry("imageConfig".to_owned())
    .or_insert_with(|| serde_json::json!({}))
    .as_object_mut()
    .ok_or_else(|| Error::Build("Gemini imageConfig must be an object".into()))?;
  for (key, value) in options {
    config.insert(
      if key == "aspect_ratio" { "aspectRatio" } else { "imageSize" }.into(),
      value.clone(),
    );
  }

  Ok(defaults)
}

pub(crate) fn submission(
  model: &str,
  prompt: &str,
  parameters: &Map<String, Value>,
  images: &[crate::extensions::shared::images::ReferenceInput],
) -> Result<crate::protocol::endpoint::Draft, Error> {
  validate(model, parameters)?;
  let mut parts = vec![serde_json::json!({"text":prompt})];
  for image in images {
    let crate::extensions::shared::images::ReferenceInput::Inline(image) = image else {
      return Err(Error::Build("Gemini requires a session image blob".into()));
    };
    parts.push(serde_json::json!({"inlineData":{"mimeType":image.mime,"data":image.data}}));
  }
  crate::protocol::endpoint::Draft::post_json(
    &serde_json::json!({"contents":[{"role":"user","parts":parts}],"generationConfig":parameters}),
  )
}

pub fn capabilities(model: &str) -> crate::extensions::shared::images::ImageCapabilities {
  crate::extensions::shared::images::ImageCapabilities {
    reference_limit: 14,
    local_references: true,
    remote_references: false,
    image_required: false,
    mask: false,
    markup: false,
    options: request_options(model),
  }
}
