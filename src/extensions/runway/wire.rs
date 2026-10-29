//! Runway wire formats. Shared host code supplies resolved assets and credentials.
use super::video_options::VideoOptions;
use crate::extensions::shared::video::{InputRole, ResolvedInput};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};
pub(crate) fn generation(
  model: &str,
  prompt: &str,
  options: &VideoOptions,
) -> Result<(String, Value), Error> {
  let invalid = |s: &str| Error::Build(s.into());
  if prompt.trim().is_empty() {
    return Err(invalid("Runway prompt must not be empty"));
  }

  let duration = options.duration.unwrap_or(5.0);
  if model != "gen4.5" || !(2.0..=10.0).contains(&duration) || duration.fract() != 0.0 {
    return Err(invalid("Runway gen4.5 requires an integer duration of 2..10 seconds"));
  }
  if prompt.encode_utf16().count() > 1000 {
    return Err(invalid("Runway prompt exceeds 1000 UTF-16 characters"));
  }
  if options.resolution.as_deref().is_some_and(|v| !v.eq_ignore_ascii_case("720p")) {
    return Err(invalid("Runway gen4.5 supports 720p output"));
  }
  let ratio = match options.aspect_ratio.as_deref().unwrap_or("16:9") {
    "16:9" => "1280:720",
    "9:16" => "720:1280",
    _ => return Err(invalid("Runway gen4.5 supports 16:9 or 9:16")),
  };
  Ok((
    "/v1/text_to_video".into(),
    json!({"model":model,"promptText":prompt,"duration":duration as u64,"ratio":ratio}),
  ))
}
pub(crate) fn edit(
  model: &str,
  prompt: &str,
  options: &VideoOptions,
  inputs: &[ResolvedInput],
) -> Result<(String, Draft), Error> {
  use InputRole::{FirstFrame, LastFrame, SourceVideo};
  let invalid = |s: &str| Error::Build(s.into());
  let uri = |role| inputs.iter().find(|i| i.role == role).map(|i| i.uri.as_str());

  if model != "aleph2"
    || options.duration.is_some()
    || options.resolution.is_some()
    || options.aspect_ratio.is_some()
  {
    return Err(invalid(
      "Aleph 2 editing preserves input length/framing; omit duration, resolution and aspect_ratio",
    ));
  }
  let mut body = json!({"model":model,"promptText":prompt,"videoUri":uri(SourceVideo).ok_or_else(|| invalid("missing source_video"))?});
  let frames: Vec<_> = inputs
    .iter()
    .filter(|i| matches!(i.role, FirstFrame | LastFrame))
    .map(|i| json!({"uri":i.uri,"at":if i.role==FirstFrame {0.0} else {1.0}}))
    .collect();
  if !frames.is_empty() {
    body["keyframes"] = json!(frames);
  }
  Ok(("/v1/video_to_video".into(), Draft::post_json(&body)?))
}

pub(crate) fn with_inputs(
  model: &str,
  prompt: &str,
  mode: crate::extensions::shared::video::VideoMode,
  options: &VideoOptions,
  inputs: &[ResolvedInput],
) -> Result<(String, Draft), Error> {
  use crate::extensions::shared::video::{InputRole::*, VideoMode::*};
  if mode == Edit {
    return edit(model, prompt, options, inputs);
  }
  let (mut path, mut body) = generation(model, prompt, options)?;
  if mode == Frames {
    path = "/v1/image_to_video".into();
    body["promptImage"] = json!(
      inputs
        .iter()
        .find(|i| i.role == FirstFrame)
        .map(|i| i.uri.as_str())
        .ok_or_else(|| Error::Build("missing first_frame".into()))?
    );
  }
  Ok((path, Draft::post_json(&body)?))
}

pub fn validate_configuration(
  model: &str,
  values: &crate::extensions::shared::parameters::Parameters,
) -> Result<(), Error> {
  let options = crate::extensions::shared::parameters::decode(values)?;
  if model == "aleph2" {
    edit(
      model,
      "validation",
      &options,
      &[ResolvedInput {
        role: InputRole::SourceVideo,
        uri: "https://placeholder.invalid/input".into(),
        timestamp_s: None,
        voice_id: None,
      }],
    )
    .map(|_| ())
  } else {
    generation(model, "validation", &options).map(|_| ())
  }
}
