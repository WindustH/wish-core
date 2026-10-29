//! Supplier-owned request validation and wire encoding.
use crate::protocol::error::Error;
use serde_json::{Value, json};
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
  pub duration: Option<f64>,
  pub aspect_ratio: Option<String>,
  pub resolution: Option<String>,
}
pub(crate) fn submission(
  model: &str,
  prompt: &str,
  options: &Options,
  version25: bool,
) -> Result<(String, Value), Error> {
  let invalid = |s: &str| Error::Build(s.into());
  if prompt.trim().is_empty() || prompt.chars().count() > 4100 {
    return Err(invalid("prompt is empty or exceeds the service limit"));
  }
  Ok({
    let max_duration = if version25 { 30.0 } else { 15.0 };
    let duration = options.duration.unwrap_or(5.0);
    if duration != -1.0 && (!(4.0..=max_duration).contains(&duration) || duration.fract() != 0.0) {
      return Err(invalid(
        "Seedance duration must be -1 (automatic), or an integer in 4..15 (2.5: 4..30)",
      ));
    }
    let resolution = options.resolution.as_deref().unwrap_or("720p").to_ascii_lowercase();
    let resolutions: &[&str] =
      if version25 { &["480p", "720p", "1080p"] } else { &["480p", "720p", "1080p", "4k"] };
    if !resolutions.contains(&resolution.as_str()) {
      return Err(invalid("unsupported Seedance resolution"));
    }
    let ratio = options.aspect_ratio.as_deref().unwrap_or("16:9");
    if !["16:9", "4:3", "1:1", "3:4", "9:16", "21:9", "adaptive"].contains(&ratio) {
      return Err(invalid("unsupported Seedance aspect ratio"));
    }
    (
      "/api/v3/contents/generations/tasks".into(),
      json!({"model":model,"content":[{"type":"text","text":prompt}],"duration":duration as i64,"resolution":resolution,"ratio":ratio}),
    )
  })
}

pub(crate) fn with_inputs(
  model: &str,
  prompt: &str,
  mode: crate::extensions::shared::video::VideoMode,
  options: &Options,
  inputs: &[crate::extensions::shared::video::ResolvedInput],
  version25: bool,
) -> Result<(String, crate::protocol::endpoint::Draft), Error> {
  use crate::extensions::shared::video::{InputRole::*, VideoMode::*};
  let (path, mut body) = submission(model, prompt, options, version25)?;
  let content =
    body["content"].as_array_mut().ok_or_else(|| Error::Build("missing video content".into()))?;
  for input in inputs {
    let kind = format!("{}_url", input.role.media_type());
    let role = if input.role == SourceVideo { ReferenceVideo } else { input.role };
    content.push(json!({"type":kind.clone(),kind:{"url":input.uri},"role":role}));
  }
  if version25 && matches!(mode, Reference | Edit | Extend) {
    body["omni_reference_task_type"] = json!(mode);
  }
  Ok((path, crate::protocol::endpoint::Draft::post_json(&body)?))
}
