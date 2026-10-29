//! Gemini Interactions image request envelope; image format is not a generation-config field.
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Map, Value, json};

pub(crate) fn submission(
  model: &str,
  input: Vec<Value>,
  parameters: &Map<String, Value>,
) -> Result<Draft, Error> {
  let mut generation = parameters.clone();
  let mut format = match generation.remove("response_format") {
    None => Map::new(),
    Some(Value::Object(format)) => format,
    _ => return Err(Error::Build("Gemini image response_format must be an object".into())),
  };
  for key in ["aspect_ratio", "image_size"] {
    if let Some(value) = generation.remove(key) {
      format.insert(key.into(), value);
    }
  }
  if format.get("aspect_ratio").is_some_and(|v| {
    !v.as_str().is_some_and(|v| {
      [
        "1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9", "1:8", "8:1",
        "1:4", "4:1",
      ]
      .contains(&v)
    })
  }) {
    return Err(Error::Build("unsupported Gemini image aspect_ratio".into()));
  }
  if format
    .get("image_size")
    .is_some_and(|v| !v.as_str().is_some_and(|v| ["1K", "2K", "4K"].contains(&v)))
  {
    return Err(Error::Build("Gemini Nano Banana image_size must be 1K, 2K or 4K".into()));
  }
  if format.get("mime_type").is_some_and(|v| v != "image/jpeg") {
    return Err(Error::Build(
      "Gemini Interactions image output MIME type must be image/jpeg".into(),
    ));
  }
  if format.get("type").is_some_and(|v| v != "image")
    || format.get("delivery").is_some_and(|v| v != "inline")
  {
    return Err(Error::Build("Wish requires inline image output from Gemini Interactions".into()));
  }
  if format.keys().any(|key| {
    !["type", "delivery", "mime_type", "aspect_ratio", "image_size"].contains(&key.as_str())
  }) {
    return Err(Error::Build("unknown Gemini image response_format field".into()));
  }
  format.insert("type".into(), json!("image"));
  format.insert("delivery".into(), json!("inline"));
  Draft::post_json(
    &json!({"model":model,"input":input,"generation_config":generation,"response_format":format}),
  )
}

use crate::extensions::shared::images::ImageSource;
pub(crate) fn collect_images(value: &Value, images: &mut Vec<ImageSource>) {
  // New image-only responses expose the final artifact directly; do not also collect the
  // same image from steps. Never walk arbitrary content (user inputs/tool outputs/thoughts).
  if let Some(data) = value["output_image"]["data"].as_str() {
    images.push(ImageSource::Base64(data.into()));
    return;
  }
  let mut collect_content = |content: &Value| {
    if let Some(items) = content.as_array() {
      for item in items {
        if item["type"] == "image"
          && let Some(data) = item["data"].as_str()
        {
          images.push(ImageSource::Base64(data.into()));
        }
      }
    }
  };
  if let Some(steps) = value["steps"].as_array() {
    for step in steps {
      if step["type"] == "model_output" {
        collect_content(&step["content"]);
      }
    }
  } else {
    collect_content(&value["outputs"]);
  }
}

pub(crate) fn with_images(
  model: &str,
  prompt: &str,
  parameters: &serde_json::Map<String, Value>,
  images: &[crate::extensions::shared::images::ReferenceInput],
) -> Result<crate::protocol::endpoint::Draft, Error> {
  let mut input = vec![serde_json::json!({"type":"text","text":prompt})];
  for image in images {
    let crate::extensions::shared::images::ReferenceInput::Inline(image) = image else {
      return Err(Error::Build("Gemini Interactions requires a session image blob".into()));
    };
    input.push(serde_json::json!({"type":"image","mime_type":image.mime,"data":image.data}));
  }
  submission(model, input, parameters)
}
pub(crate) fn decode(body: &Value) -> Result<crate::extensions::shared::images::ImageReply, Error> {
  let mut images = Vec::new();
  collect_images(body, &mut images);
  if images.is_empty() || images.len() > 8 {
    return Err(Error::Malformed(
      "Interactions response contains no supported images or exceeds 8 images".into(),
    ));
  }
  Ok(crate::extensions::shared::images::ImageReply::Complete(images))
}

pub fn capabilities(_model: &str) -> crate::extensions::shared::images::ImageCapabilities {
  crate::extensions::shared::images::ImageCapabilities {
    reference_limit: 14,
    local_references: true,
    remote_references: false,
    image_required: false,
    mask: false,
    markup: false,
    options: &["aspect_ratio", "image_size"],
  }
}
