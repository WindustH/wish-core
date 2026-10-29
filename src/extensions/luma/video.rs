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
    if !["ray-2", "ray-flash-2"].contains(&model) {
      return Err(invalid("unsupported Luma Dream Machine model"));
    }
    let duration = options.duration.unwrap_or(5.0);
    if ![5.0, 9.0].contains(&duration) {
      return Err(invalid("Luma duration must be 5 or 9 seconds"));
    }
    let resolution = options.resolution.as_deref().unwrap_or("720p").to_ascii_lowercase();
    if !["540p", "720p", "1080p", "4k"].contains(&resolution.as_str()) {
      return Err(invalid("unsupported Luma resolution"));
    }
    let ratio = options.aspect_ratio.as_deref().unwrap_or("16:9");
    if !["21:9", "16:9", "4:3", "1:1", "3:4", "9:16", "9:21"].contains(&ratio) {
      return Err(invalid("unsupported Luma aspect ratio"));
    }
    (
      "/dream-machine/v1/generations/video".into(),
      json!({"model":model,"prompt":prompt,"duration":format!("{}s",duration as u64),"resolution":resolution,"aspect_ratio":ratio}),
    )
  })
}

/// Build the supplier request after the host has resolved session-owned input assets.
pub(crate) fn with_inputs(
  model: &str,
  prompt: &str,
  mode: crate::extensions::shared::video::VideoMode,
  options: &Options,
  inputs: &[crate::extensions::shared::video::ResolvedInput],
  modify_mode: Option<&str>,
) -> Result<(String, crate::protocol::endpoint::Draft), Error> {
  use crate::extensions::shared::video::{InputRole::*, VideoMode::*};
  use crate::protocol::endpoint::Draft;
  let invalid = |text: &str| Error::Build(text.into());
  let uri = |role| inputs.iter().find(|i| i.role == role).map(|i| i.uri.as_str());
  if prompt.trim().is_empty() || prompt.chars().count() > 4100 {
    return Err(invalid("prompt must contain 1..4100 characters"));
  }
  if mode == Edit {
    if options.duration.is_some() || options.resolution.is_some() || options.aspect_ratio.is_some()
    {
      return Err(invalid("Luma editing does not accept duration/resolution/aspect_ratio"));
    }
    let modify = modify_mode.unwrap_or("flex_1");
    if ![
      "adhere_1",
      "adhere_2",
      "adhere_3",
      "flex_1",
      "flex_2",
      "flex_3",
      "reimagine_1",
      "reimagine_2",
      "reimagine_3",
    ]
    .contains(&modify)
    {
      return Err(invalid("invalid Luma modify_mode"));
    }
    if !["ray-2", "ray-flash-2"].contains(&model) {
      return Err(invalid("unsupported Luma model"));
    }
    let mut body = json!({"generation_type":"modify_video","model":model,"prompt":prompt,"mode":modify,"media":{"url":uri(SourceVideo).ok_or_else(|| invalid("missing source_video"))?}});
    if let Some(first) = uri(FirstFrame) {
      body["first_frame"] = json!({"url":first});
    }
    return Ok(("/dream-machine/v1/generations/video/modify".into(), Draft::post_json(&body)?));
  }
  let (path, mut body) = submission(model, prompt, options)?;
  if matches!(mode, Frames | Extend) {
    let mut frames = json!({});
    if let Some(first) = uri(FirstFrame) {
      frames["frame0"] = json!({"type":"image","url":first});
    }
    if let Some(last) = uri(LastFrame) {
      frames["frame1"] = json!({"type":"image","url":last});
    }
    if let Some(task) = uri(SourceVideo) {
      frames["frame0"] = json!({"type":"generation","id":task});
    }
    body["keyframes"] = frames;
  }
  Ok((path, Draft::post_json(&body)?))
}
