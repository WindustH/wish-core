//! GPT Image JSON edit endpoint; distinct from the subscription/Codex adapter.
use crate::extensions::shared::images::{EditOverlay, ReferenceInput};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Map, Value, json};

pub(crate) fn reference_limit(model: &str) -> usize {
  match model {
    "gpt-image-1.5"
    | "gpt-image-1-mini"
    | "gpt-image-2"
    | "gpt-image-2-2026-04-21"
    | "gpt-image-2.5-sunburst"
    | "gpt-image-2.5-sunburst-2026-09-08"
    | "gpt-image-2.5-flare"
    | "gpt-image-2.5-flare-2026-09-08"
    | "chatgpt-image-latest" => 16,
    _ => 0,
  }
}

pub(crate) fn markup_supported(model: &str) -> bool {
  matches!(
    model,
    "gpt-image-2.5-sunburst"
      | "gpt-image-2.5-sunburst-2026-09-08"
      | "gpt-image-2.5-flare"
      | "gpt-image-2.5-flare-2026-09-08"
  )
}
pub(crate) fn edit(
  model: &str,
  prompt: &str,
  parameters: &Map<String, Value>,
  images: &[ReferenceInput],
  overlay: Option<&EditOverlay>,
) -> Result<Draft, Error> {
  if images.is_empty() || images.len() > reference_limit(model) {
    return Err(Error::Build("unsupported GPT Image edit model or reference count".into()));
  }
  if prompt.chars().count() > 32000 {
    return Err(Error::Build("OpenAI image edit prompt exceeds 32000 characters".into()));
  }
  validate_parameters(model, parameters)?;
  let mut args = parameters.clone();
  args.insert("model".into(), json!(model));
  args.insert("prompt".into(), json!(prompt));
  args.insert(
    "images".into(),
    json!(
      images
        .iter()
        .map(|image| {
          let url = match image {
            ReferenceInput::Inline(image) => format!("data:{};base64,{}", image.mime, image.data),
            ReferenceInput::Url(url) => url.clone(),
          };
          json!({"image_url":url})
        })
        .collect::<Vec<_>>()
    ),
  );
  if let Some(overlay) = overlay {
    let image = overlay.image();
    args.insert(
      overlay.field().into(),
      json!({"image_url":format!("data:{};base64,{}",image.mime,image.data)}),
    );
  }
  let mut draft = Draft::post_json(&Value::Object(args))?;
  draft.path = Some("/images/edits".into());
  Ok(draft)
}

pub(crate) fn request_options(model: &str) -> &'static [&'static str] {
  if reference_limit(model) == 0 {
    return &[];
  }
  if matches!(model, "gpt-image-1.5" | "gpt-image-1-mini") {
    &[
      "size",
      "quality",
      "background",
      "output_format",
      "output_compression",
      "n",
      "moderation",
      "input_fidelity",
    ]
  } else {
    &["size", "quality", "background", "output_format", "output_compression", "n", "moderation"]
  }
}

pub(crate) fn validate_parameters(
  model: &str,
  parameters: &Map<String, Value>,
) -> Result<(), Error> {
  // OpenAI-compatible presets can use other vendors' models and parameter contracts.
  if reference_limit(model) == 0 {
    return Ok(());
  }
  let value = |key: &str| parameters.get(key).filter(|v| !v.is_null());
  for (key, allowed) in [
    ("background", &["auto", "opaque", "transparent"][..]),
    ("output_format", &["png", "jpeg", "webp"][..]),
    ("moderation", &["auto", "low"][..]),
  ] {
    if value(key).is_some_and(|v| !v.as_str().is_some_and(|v| allowed.contains(&v))) {
      return Err(Error::Build(format!("invalid GPT Image {key}")));
    }
  }
  if value("quality").is_some_and(|v| {
    !v.as_str().is_some_and(|q| {
      ["auto", "low", "medium", "high"].contains(&q)
        || (markup_supported(model) && ["xhigh", "max"].contains(&q))
    })
  }) {
    return Err(Error::Build("quality is unsupported by this GPT Image model".into()));
  }
  if value("input_fidelity").is_some_and(|v| {
    !matches!(
      (model, v.as_str()),
      ("gpt-image-1.5", Some("low" | "high")) | ("gpt-image-1-mini", Some("low"))
    )
  }) {
    return Err(Error::Build("input_fidelity is unsupported by this GPT Image model".into()));
  }
  if value("background").and_then(Value::as_str) == Some("transparent")
    && value("output_format").and_then(Value::as_str) == Some("jpeg")
  {
    return Err(Error::Build("transparent output requires png or webp".into()));
  }
  if value("n").is_some_and(|v| !v.as_u64().is_some_and(|n| (1..=8).contains(&n))) {
    return Err(Error::Build("image count must be between 1 and 8".into()));
  }
  if value("output_compression").is_some_and(|v| !v.as_u64().is_some_and(|n| n <= 100)) {
    return Err(Error::Build("output_compression must be an integer in 0..100".into()));
  }
  if value("output_compression").is_some()
    && !matches!(value("output_format").and_then(Value::as_str), Some("jpeg" | "webp"))
  {
    return Err(Error::Build("output_compression requires jpeg or webp output".into()));
  }
  if value("size").is_some_and(|v| !v.as_str().is_some_and(|size| valid_size(model, size))) {
    return Err(Error::Build("size is unsupported by this GPT Image model".into()));
  }
  Ok(())
}
fn valid_size(model: &str, size: &str) -> bool {
  if matches!(size, "auto" | "1024x1024" | "1536x1024" | "1024x1536") {
    return true;
  }
  if !markup_supported(model) && !matches!(model, "gpt-image-2" | "gpt-image-2-2026-04-21") {
    return false;
  }
  let Some((width, height)) = size.split_once('x') else {
    return false;
  };
  let (Ok(width), Ok(height)) = (width.parse::<u64>(), height.parse::<u64>()) else {
    return false;
  };
  width > 0
    && height > 0
    && width <= 3840
    && height <= 3840
    && width % 16 == 0
    && height % 16 == 0
    && width <= height * 3
    && height <= width * 3
    && (655360..=8294400).contains(&(width * height))
}

pub(crate) fn generation(
  model: &str,
  prompt: &str,
  parameters: &serde_json::Map<String, serde_json::Value>,
) -> Result<crate::protocol::endpoint::Draft, crate::protocol::error::Error> {
  let mut args = parameters.clone();
  validate_parameters(model, parameters)?;
  args.remove("input_fidelity");
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

pub fn capabilities(model: &str) -> crate::extensions::shared::images::ImageCapabilities {
  crate::extensions::shared::images::ImageCapabilities {
    reference_limit: reference_limit(model),
    local_references: true,
    remote_references: true,
    image_required: false,
    mask: true,
    markup: markup_supported(model),
    options: request_options(model),
  }
}
