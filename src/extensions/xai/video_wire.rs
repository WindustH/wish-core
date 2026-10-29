//! Grok Imagine video wire contract; durable polling and blob storage live above.
use super::video_options::VideoOptions;
use crate::extensions::shared::video::{InputRole, ResolvedInput, VideoMode};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::json;
fn invalid(message: &str) -> Error {
  Error::Build(message.into())
}
pub(crate) fn submission(
  model: &str,
  prompt: &str,
  mode: VideoMode,
  options: &VideoOptions,
  inputs: &[ResolvedInput],
) -> Result<(String, Draft), Error> {
  use InputRole::*;
  use VideoMode::*;
  let image_led = matches!(mode, Frames | Reference)
    && inputs.iter().any(|i| {
      i.role == FirstFrame
        || (model == "grok-imagine-video-1.5"
          && matches!(i.role, LastFrame | Keyframe | ReferenceImage))
    });
  if (!image_led && prompt.trim().is_empty()) || prompt.chars().count() > 4100 {
    return Err(invalid("prompt must contain 1..4100 characters"));
  }
  let modern = model == "grok-imagine-video-1.5";
  let lite = model == "grok-imagine-video-1.5-lite";
  if !modern && !lite && model != "grok-imagine-video" {
    return Err(invalid("unsupported xAI video model"));
  }
  let mut body = json!({"model":model});
  if !prompt.trim().is_empty() {
    body["prompt"] = json!(prompt);
  }
  let source = |role| inputs.iter().find(|i| i.role == role).map(|i| json!({"url":i.uri}));
  let route = match mode {
    Edit | Extend => {
      if modern || lite || inputs.len() != 1 || source(SourceVideo).is_none() {
        return Err(invalid("xAI edit/extend requires grok-imagine-video and one source_video"));
      }
      if options.aspect_ratio.is_some()
        || options.resolution.is_some()
        || (mode == Edit && options.duration.is_some())
      {
        return Err(invalid(
          "xAI edit/extend preserves source framing; edit also preserves duration",
        ));
      }
      body["video"] = source(SourceVideo).ok_or_else(|| invalid("missing xAI source video"))?;
      if mode == Extend {
        let duration = options.duration.unwrap_or(6.0);
        if !(2.0..=10.0).contains(&duration) || duration.fract() != 0.0 {
          return Err(invalid("xAI extension duration must be an integer from 2 to 10"));
        }
        body["duration"] = json!(duration as u64);
        "/v1/videos/extensions"
      } else {
        "/v1/videos/edits"
      }
    }
    _ => {
      let references = inputs
        .iter()
        .filter(|i| i.role == ReferenceImage)
        .map(|i| json!({"url":i.uri}))
        .collect::<Vec<_>>();
      let voices = inputs
        .iter()
        .filter(|i| i.role == ReferenceVoice)
        .map(|i| {
          i.voice_id
            .as_ref()
            .map(|v| json!({"voice_id":v}))
            .ok_or_else(|| invalid("missing preset voice_id"))
        })
        .collect::<Result<Vec<_>, _>>()?;
      if voices.len() > 3 || (!modern && !voices.is_empty()) {
        return Err(invalid("preset voice references require xAI 1.5 and at most three voices"));
      }
      if !voices.is_empty() {
        body["reference_audios"] = json!(voices);
      }
      let keyframes: Vec<_> = inputs.iter().filter(|i| i.role == Keyframe).collect();
      let reference_mode = !references.is_empty()
        || source(LastFrame).is_some()
        || !keyframes.is_empty()
        || !voices.is_empty();
      if references.len() > if modern { 14 } else { 7 }
        || keyframes.len() > 4
        || (!modern && !keyframes.is_empty())
        || (!modern && source(LastFrame).is_some())
        || (!modern && reference_mode && source(FirstFrame).is_some())
      {
        return Err(invalid("xAI reference/frame combination exceeds this model's capabilities"));
      }
      if lite && reference_mode {
        return Err(invalid("xAI Lite supports text or first-image generation only"));
      }
      let max_duration = if !modern && reference_mode { 10.0 } else { 15.0 };
      let duration = options.duration.unwrap_or(8.0);
      if !(1.0..=max_duration).contains(&duration) || duration.fract() != 0.0 {
        return Err(invalid("xAI video duration exceeds this model/mode range"));
      }
      let mut slots = std::collections::HashSet::new();
      for frame in &keyframes {
        let time = frame.timestamp_s.ok_or_else(|| invalid("keyframe requires timestamp_s"))?;
        if !time.is_finite()
          || time <= 0.0
          || time >= duration
          || !slots.insert((time * 3.0).round() as u32)
        {
          return Err(invalid(
            "keyframe timestamps must be inside duration and occupy distinct 1/3-second slots",
          ));
        }
      }
      if !keyframes.is_empty() {
        body["keyframes"] = json!(
          keyframes
            .iter()
            .map(|i| json!({"image":{"url":i.uri},"timestamp_s":i.timestamp_s}))
            .collect::<Vec<_>>()
        );
      }
      let resolution = options.resolution.as_deref().unwrap_or("480p");
      if !["480p", "720p"].contains(&resolution)
        && !(resolution == "1080p" && (modern || lite) && !reference_mode)
      {
        return Err(invalid("unsupported xAI resolution for this model/mode"));
      }
      body["duration"] = json!(duration as u64);
      body["resolution"] = json!(resolution);
      if let Some(ratio) = options.aspect_ratio.as_deref() {
        if source(FirstFrame).is_some() && !reference_mode {
          return Err(invalid(
            "xAI image-to-video preserves the source aspect ratio; omit aspect_ratio",
          ));
        }
        if !["1:1", "16:9", "9:16", "4:3", "3:4", "3:2", "2:3", "21:9", "5:2"].contains(&ratio) {
          return Err(invalid("unsupported xAI video aspect_ratio"));
        }
        body["aspect_ratio"] = json!(ratio);
      }
      if let Some(image) = source(FirstFrame) {
        body["image"] = image;
      }
      if let Some(image) = source(LastFrame) {
        body["last_frame"] = image;
      }
      if !references.is_empty() {
        body["reference_images"] = json!(references);
      }
      "/v1/videos/generations"
    }
  };
  if let Some(enabled) = options.generate_audio {
    if matches!(mode, Edit | Extend) {
      return Err(invalid("generate_audio is only supported by xAI video generation"));
    }
    body["generate_audio"] = json!(enabled);
  }
  Ok((route.into(), Draft::post_json(&body)?))
}

/// Capability metadata belongs to the same extension as the wire constraints.
pub(crate) fn capabilities(
  model: &str,
) -> Option<crate::extensions::shared::video::VideoCapabilities> {
  use crate::extensions::shared::video::{InputRole::*, VideoCapabilities, VideoMode::*};
  let (modes, input_roles, blobs, notes) = match model {
    "grok-imagine-video-1.5" => (
      vec![Text, Frames, Reference],
      vec![FirstFrame, LastFrame, Keyframe, ReferenceImage, ReferenceVoice],
      vec!["image"],
      "Grok Imagine 1.5: up to 14 image references, optionally combined with first/last frames and up to 4 keyframes (timestamp_s inside duration, distinct rounded 1/3-second slots); 1..15s. Text/first-frame animation supports 1080p; references/last-frame cap at 720p. Up to 3 reference_voice inputs with preset voice_id (e.g. eve, leo); refer to them as <AUDIO_0> etc. Custom audio references require special upstream access and are not supported. Local image data URLs supported.",
    ),
    "grok-imagine-video-1.5-lite" => (
      vec![Text, Frames],
      vec![FirstFrame],
      vec!["image"],
      "Grok Imagine 1.5 Lite: text or one first-image input, 1..15s, 480p/720p/1080p (1080p upscaled from720p). No references, preset voices, last frame, keyframes, editing or extension. Image input preserves framing.",
    ),
    "grok-imagine-video" => (
      vec![Text, Frames, Reference, Edit, Extend],
      vec![FirstFrame, ReferenceImage, SourceVideo],
      vec!["image", "video"],
      "Classic Grok Imagine: up to 7 image references, no mixing with first frame; references up to 10s, generation up to 15s, 720p maximum. Edit source up to 8.7s; extend source 2..15s by another 2..10s. Source duration/codec checked upstream. Local MP4 data URLs supported; no framing controls in edit/extend.",
    ),

    _ => return None,
  };
  Some(VideoCapabilities {
    modes,
    input_roles,
    blob_inputs: blobs.into_iter().map(str::to_owned).collect(),
    notes: notes.into(),
  })
}
