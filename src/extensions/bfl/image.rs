//! BFL reference input fields and per-model limits from https://api.bfl.ai/openapi.json.
use crate::extensions::shared::images::ReferenceInput;
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Map, Value, json};
pub(crate) fn request_options(model: &str) -> &'static [&'static str] {
  match model {
    "flux-2-pro" | "flux-2-pro-preview" | "flux-2-max" => {
      &["width", "height", "seed", "output_format", "prompt_upsampling"]
    }
    "flux-2-flex" => {
      &["width", "height", "seed", "output_format", "prompt_upsampling", "steps", "guidance"]
    }
    "flux-2-klein-4b" | "flux-2-klein-9b" | "flux-2-klein-9b-preview" => {
      &["width", "height", "seed", "output_format"]
    }
    "flux-kontext-pro" | "flux-kontext-max" => {
      &["aspect_ratio", "seed", "output_format", "prompt_upsampling"]
    }
    "flux-pro-1.0-fill" => &["seed", "output_format", "prompt_upsampling", "steps", "guidance"],
    _ => &[],
  }
}
pub(crate) fn validate(model: &str, parameters: &Map<String, Value>) -> Result<(), Error> {
  if model == "flux-pro-1.0-fill" {
    return validate_fill_parameters(parameters);
  }
  if reference_limit(model) == 0 {
    return Ok(());
  }
  let value = |key: &str| parameters.get(key).filter(|v| !v.is_null());
  for key in ["width", "height"] {
    if value(key).is_some_and(|v| !v.as_u64().is_some_and(|n| n == 0 || n >= 64)) {
      return Err(Error::Build(format!(
        "BFL {key} must be 0 (automatic) or an integer at least 64"
      )));
    }
  }
  if value("seed").is_some_and(|v| !v.is_i64() && !v.is_u64()) {
    return Err(Error::Build("BFL seed must be an integer".into()));
  }
  for key in ["prompt_upsampling", "disable_pup"] {
    if value(key).is_some_and(|v| !v.is_boolean()) {
      return Err(Error::Build(format!("BFL {key} must be boolean")));
    }
  }
  if value("output_format").is_some_and(|v| !matches!(v.as_str(), Some("png" | "jpeg" | "webp"))) {
    return Err(Error::Build("BFL output_format must be png, jpeg or webp".into()));
  }
  if model == "flux-2-flex" {
    if value("steps").is_some_and(|v| !v.as_u64().is_some_and(|n| (1..=50).contains(&n))) {
      return Err(Error::Build("BFL Flex steps must be an integer in 1..50".into()));
    }
    if value("guidance").is_some_and(|v| !v.as_f64().is_some_and(|n| (1.5..=10.0).contains(&n))) {
      return Err(Error::Build("BFL Flex guidance must be in 1.5..10".into()));
    }
  }
  if model.starts_with("flux-kontext-")
    && let Some(ratio) = value("aspect_ratio")
  {
    let ratio = ratio.as_str().and_then(|s| s.split_once(':')).and_then(|(w, h)| {
      let (w, h) = (w.parse::<f64>().ok()?, h.parse::<f64>().ok()?);
      (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some(w / h)
    });
    if !ratio.is_some_and(|r| (9.0 / 21.0..=21.0 / 9.0).contains(&r)) {
      return Err(Error::Build("BFL Kontext aspect_ratio must be between 9:21 and 21:9".into()));
    }
  }
  Ok(())
}
pub(crate) fn reference_limit(model: &str) -> usize {
  match model {
    "flux-2-pro" | "flux-2-pro-preview" | "flux-2-max" | "flux-2-flex" => 8,
    "flux-2-klein-4b"
    | "flux-2-klein-9b"
    | "flux-2-klein-9b-preview"
    | "flux-kontext-pro"
    | "flux-kontext-max" => 4,
    "flux-pro-1.0-fill" => 1,
    _ => 0,
  }
}
pub(crate) fn edit(
  model: &str,
  prompt: &str,
  parameters: &Map<String, Value>,
  images: &[ReferenceInput],
) -> Result<Draft, Error> {
  if images.len() > reference_limit(model) {
    return Err(Error::Build("unsupported BFL reference count or model".into()));
  }
  let mut args = parameters.clone();
  if args.keys().any(|key| key.starts_with("input_image")) {
    return Err(Error::Build(
      "BFL input images must be supplied through the images argument".into(),
    ));
  }
  args.insert("prompt".into(), json!(prompt));
  for (index, image) in images.iter().enumerate() {
    let data = match image {
      ReferenceInput::Inline(image) => &image.data,
      ReferenceInput::Url(url) => url,
    };
    let key = if index == 0 { "input_image".into() } else { format!("input_image_{}", index + 1) };
    args.insert(key, json!(data));
  }
  Draft::post_json(&Value::Object(args))
}

pub(crate) fn fill(
  prompt: &str,
  parameters: &Map<String, Value>,
  images: &[ReferenceInput],
  mask: Option<&crate::extensions::shared::images::ReferenceImage>,
) -> Result<Draft, Error> {
  let [ReferenceInput::Inline(image)] = images else {
    return Err(Error::Build("BFL Fill requires exactly one session image blob".into()));
  };
  let mut args = parameters.clone();
  args.insert("prompt".into(), json!(prompt));
  args.insert("image".into(), json!(image.data));
  if let Some(mask) = mask {
    args.insert("mask".into(), json!(mask.data));
  }
  validate_fill_parameters(parameters)?;
  Draft::post_json(&Value::Object(args))
}

pub(crate) fn validate_fill_parameters(parameters: &Map<String, Value>) -> Result<(), Error> {
  // Optional nullable fields follow the upstream schema; null selects the server default.
  let value = |key: &str| parameters.get(key).filter(|v| !v.is_null());
  if value("steps").is_some_and(|v| !v.as_u64().is_some_and(|v| (15..=50).contains(&v))) {
    return Err(Error::Build("BFL Fill steps must be an integer in 15..50".into()));
  }
  if value("guidance").is_some_and(|v| !v.as_f64().is_some_and(|v| (1.5..=100.0).contains(&v))) {
    return Err(Error::Build("BFL Fill guidance must be in 1.5..100".into()));
  }
  if value("prompt_upsampling").is_some_and(|v| !v.is_boolean()) {
    return Err(Error::Build("BFL Fill prompt_upsampling must be a boolean".into()));
  }
  if value("seed").is_some_and(|v| !v.is_i64() && !v.is_u64()) {
    return Err(Error::Build("BFL Fill seed must be an integer".into()));
  }
  if value("output_format").is_some_and(|v| !matches!(v.as_str(), Some("png" | "jpeg" | "webp"))) {
    return Err(Error::Build("BFL Fill output_format must be png, jpeg or webp".into()));
  }
  if parameters.get("safety_tolerance").is_some_and(|v| !v.as_u64().is_some_and(|v| v <= 6)) {
    return Err(Error::Build("BFL Fill safety_tolerance must be an integer in 0..6".into()));
  }
  Ok(())
}

pub(crate) fn decode(
  body: &Value,
  current_url: &str,
  _result_url: Option<&str>,
) -> Result<crate::extensions::shared::images::ImageReply, Error> {
  let mut images = Vec::new();
  {
    if let Some(url) = body["result"]["sample"].as_str() {
      images.push(crate::extensions::shared::images::ImageSource::Url(url.into()));
    } else if matches!(
      body["status"].as_str(),
      Some("Error" | "Failed" | "Request Moderated" | "Content Moderated")
    ) {
      return Err(Error::Malformed("image generation failed or was moderated".into()));
    } else if body["id"].is_string()
      || matches!(body["status"].as_str(), Some("Pending" | "Processing"))
    {
      return Ok(crate::extensions::shared::images::ImageReply::Pending {
        url: body["polling_url"].as_str().unwrap_or(current_url).into(),
        result_url: None,
      });
    }
  }
  if images.is_empty() || images.len() > 8 {
    return Err(Error::Malformed("no supported images or too many images".into()));
  }
  Ok(crate::extensions::shared::images::ImageReply::Complete(images))
}

/// Merge this supplier's public per-call controls into its own wire configuration.
pub(crate) fn merge_options(
  model: &str,
  mut defaults: serde_json::Map<String, serde_json::Value>,
  options: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, crate::protocol::error::Error> {
  if !matches!(model, "flux-2-pro" | "flux-2-pro-preview" | "flux-2-max") {
    defaults.extend(options.clone());
    return Ok(defaults);
  }
  defaults.extend(options.clone());
  if let Some(value) = defaults.remove("prompt_upsampling") {
    let enabled = value
      .as_bool()
      .ok_or_else(|| Error::Build("BFL prompt_upsampling must be boolean".into()))?;
    defaults.insert("disable_pup".into(), json!(!enabled));
  }

  Ok(defaults)
}

pub(crate) fn generation(
  model: &str,
  prompt: &str,
  parameters: &serde_json::Map<String, serde_json::Value>,
) -> Result<crate::protocol::endpoint::Draft, crate::protocol::error::Error> {
  let mut args = parameters.clone();
  validate(model, parameters)?;
  args.insert("prompt".into(), serde_json::json!(prompt));
  let _ = model;
  crate::protocol::endpoint::Draft::post_json(&serde_json::Value::Object(args))
}

pub fn capabilities(model: &str) -> crate::extensions::shared::images::ImageCapabilities {
  crate::extensions::shared::images::ImageCapabilities {
    reference_limit: reference_limit(model),
    local_references: true,
    remote_references: reference_limit(model) > 1,
    image_required: model == "flux-pro-1.0-fill",
    mask: model == "flux-pro-1.0-fill",
    markup: false,
    options: request_options(model),
  }
}
