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
) -> Result<(String, Value), Error> {
  let invalid = |s: &str| Error::Build(s.into());
  if prompt.trim().is_empty() || prompt.chars().count() > 4100 {
    return Err(invalid("prompt is empty or exceeds the service limit"));
  }
  Ok({
    let min = if model == "MiniMax-H3-Max" { 5.0 } else { 4.0 };
    let duration = options.duration.unwrap_or(5.0);
    if !(min..=15.0).contains(&duration) || duration.fract() != 0.0 {
      return Err(invalid("H3 duration must be an integer from 4 to 15 (Max: 5 to 15)"));
    }
    let resolution = options.resolution.as_deref().unwrap_or("768P");
    let resolutions: &[&str] = match model {
      "MiniMax-H3" => &["768P", "2K"],
      "MiniMax-H3-Highspeed" => &["768P"],
      "MiniMax-H3-Lite" | "MiniMax-H3-Max" => &["480P", "768P"],
      _ => return Err(invalid("unsupported H3 model")),
    };
    if !resolutions.contains(&resolution) {
      return Err(invalid("resolution is not supported by this H3 model"));
    }
    let ratio = options.aspect_ratio.as_deref().unwrap_or("16:9");
    if !["21:9", "16:9", "4:3", "1:1", "3:4", "9:16"].contains(&ratio) {
      return Err(invalid("unsupported video aspect_ratio"));
    }
    (
      "/v2/video_generation".into(),
      json!({"model":model,"content":[{"type":"text","text":prompt}],"duration":duration as u64,"resolution":resolution,"ratio":ratio}),
    )
  })
}

pub(crate) fn with_inputs(
  model: &str,
  prompt: &str,
  mode: crate::extensions::shared::video::VideoMode,
  options: &Options,
  inputs: &[crate::extensions::shared::video::ResolvedInput],
) -> Result<(String, crate::protocol::endpoint::Draft), Error> {
  use crate::extensions::shared::video::{InputRole::*, VideoMode::*};
  let (path, mut body) = submission(model, prompt, options)?;
  let content =
    body["content"].as_array_mut().ok_or_else(|| Error::Build("missing video content".into()))?;
  for input in inputs {
    let kind = format!("{}_url", input.role.media_type());
    let role = if input.role == SourceVideo { ReferenceVideo } else { input.role };
    content.push(json!({"type":kind.clone(),kind:{"url":input.uri},"role":role}));
  }
  if mode == Frames {
    body["ratio"] = json!("adaptive");
  }
  Ok((path, crate::protocol::endpoint::Draft::post_json(&body)?))
}
