//! RunningHub Seedream 5 Pro task-based image API.
use crate::extensions::shared::images::{ImageReply, ImageSource, ReferenceInput};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Map, Value, json};
pub(crate) fn validate(model: &str, parameters: &Map<String, Value>) -> Result<(), Error> {
  if model != "seedream-v5-pro" {
    return Err(Error::Build("unsupported RunningHub image model".into()));
  }
  for (key, value) in parameters {
    let valid = match key.as_str() {
      "width" | "height" => value.as_u64().is_some_and(|v| (240..=8192).contains(&v)),
      "resolution" => matches!(value.as_str(), Some("1k" | "2k")),
      "outputFormat" => matches!(value.as_str(), Some("jpeg" | "png")),
      _ => false,
    };
    if !valid {
      return Err(Error::Build(format!("unsupported RunningHub image parameter {key}")));
    }
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
  if images.len() > 10 {
    return Err(Error::Build("RunningHub Seedream accepts at most ten images".into()));
  }
  let urls = images
    .iter()
    .map(|image| match image {
      ReferenceInput::Url(url) => Ok(url),
      _ => Err(Error::Build("RunningHub images must be uploaded before submission".into())),
    })
    .collect::<Result<Vec<_>, _>>()?;
  let mut args = parameters.clone();
  args.insert("prompt".into(), json!(prompt));
  if !urls.is_empty() {
    args.insert("imageUrls".into(), json!(urls));
  }
  let mode = if urls.is_empty() { "text-to-image" } else { "image-to-image" };
  let mut draft = Draft::post_json(&Value::Object(args))?;
  draft.path = Some(format!("/openapi/v2/{model}/{mode}"));
  Ok(draft)
}
pub(crate) fn status(task: &str) -> Result<Draft, Error> {
  if task.is_empty()
    || task.len() > 200
    || !task.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
  {
    return Err(Error::Build("invalid RunningHub image task ID".into()));
  }
  let mut draft = Draft::post_json(&json!({"taskId":task}))?;
  draft.path = Some("/openapi/v2/query".into());
  Ok(draft)
}
pub(crate) fn decode(value: &Value) -> Result<ImageReply, Error> {
  match value["status"].as_str() {
    Some("QUEUED" | "RUNNING") => {
      Ok(ImageReply::PendingTask { task_id: value["taskId"].as_str().map(str::to_owned) })
    }
    Some("SUCCESS") => {
      let images = value["results"]
        .as_array()
        .ok_or_else(|| Error::Malformed("RunningHub image result is missing".into()))?
        .iter()
        .filter(|item| {
          matches!(item["outputType"].as_str(), Some("png" | "jpeg" | "jpg" | "webp" | "image"))
        })
        .map(|item| {
          item["url"]
            .as_str()
            .map(|url| ImageSource::Url(url.into()))
            .ok_or_else(|| Error::Malformed("RunningHub image URL is missing".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
      if images.is_empty() || images.len() > 8 {
        return Err(Error::Malformed("RunningHub returned an invalid number of images".into()));
      }
      Ok(ImageReply::Complete(images))
    }
    Some("FAILED") => Err(Error::Malformed("RunningHub image generation failed".into())),
    _ => Err(Error::Malformed("unrecognized RunningHub image task status".into())),
  }
}

/// Merge this supplier's public per-call controls into its own wire configuration.
pub(crate) fn merge_options(
  mut defaults: serde_json::Map<String, serde_json::Value>,
  options: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>, crate::protocol::error::Error> {
  let dimensions = options.contains_key("width") || options.contains_key("height");
  if dimensions
    && (options.contains_key("image_size")
      || !options.contains_key("width")
      || !options.contains_key("height"))
  {
    return Err(Error::Build(
      "RunningHub options require either width and height together, or image_size".into(),
    ));
  }
  if dimensions {
    defaults.remove("resolution");
  }
  if options.contains_key("image_size") {
    defaults.remove("width");
    defaults.remove("height");
  }
  for (key, value) in options {
    match key.as_str() {
      "image_size" => {
        let size = match value.as_str() {
          Some("1K") => "1k",
          Some("2K") => "2k",
          _ => {
            return Err(Error::Build("RunningHub Seedream image_size must be 1K or 2K".into()));
          }
        };
        defaults.insert("resolution".into(), json!(size));
      }
      "output_format" => {
        defaults.insert("outputFormat".into(), value.clone());
      }
      _ => {
        defaults.insert(key.clone(), value.clone());
      }
    }
  }

  Ok(defaults)
}

pub fn capabilities(model: &str) -> crate::extensions::shared::images::ImageCapabilities {
  crate::extensions::shared::images::ImageCapabilities {
    reference_limit: if model == "seedream-v5-pro" { 10 } else { 0 },
    local_references: true,
    remote_references: true,
    image_required: false,
    mask: false,
    markup: false,
    options: if model == "seedream-v5-pro" {
      &["width", "height", "image_size", "output_format"]
    } else {
      &[]
    },
  }
}
