//! Aleph-native temporal guidance. Only resolved asset URIs cross from the host.
use super::Asset;
use super::video_options::VideoOptions;
use crate::extensions::shared::video::{InputRole, ResolvedInput};
use crate::protocol::{endpoint::Draft, error::Error};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
  pub start_seconds: u32,
  pub end_seconds: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keyframe {
  pub image: Asset,
  pub at: Option<f64>,
  pub seconds: Option<f64>,
  pub range: Option<Window>,
}
#[derive(Clone)]
pub struct Timing {
  pub at: Option<f64>,
  pub seconds: Option<f64>,
  pub range: Option<Window>,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
  Mp4,
  Prores,
  PngSequence,
  SdrRec709_10bit,
}
impl OutputFormat {
  pub(crate) fn mime(self) -> &'static str {
    match self {
      Self::Prores => "video/quicktime",
      Self::PngSequence => "application/zip",
      _ => "video/mp4",
    }
  }
}
#[derive(Clone)]
pub enum Plan {
  Generate {
    seed: Option<u32>,
  },
  Edit {
    seed: Option<u32>,
    target_aspect_ratio: Option<String>,
    frames: Vec<Timing>,
    output_format: Option<OutputFormat>,
    prores_profile: Option<String>,
  },
}
fn invalid(message: &str) -> Error {
  Error::Build(message.into())
}
impl Plan {
  pub(crate) fn artifact_mime(&self) -> Option<&'static str> {
    match self {
      Self::Edit { output_format: Some(format), .. } => Some(format.mime()),
      _ => None,
    }
  }

  pub(crate) fn build(
    &self,
    model: &str,
    prompt: &str,
    options: &VideoOptions,
    inputs: &[ResolvedInput],
  ) -> Result<(String, Draft), Error> {
    let (route, mut body, seed) = match self {
      Self::Generate { seed } => {
        if prompt.trim().is_empty() {
          return Err(invalid("Runway generation requires a prompt"));
        }
        if inputs.len() > 1 || inputs.iter().any(|i| i.role != InputRole::FirstFrame) {
          return Err(invalid("Runway generation accepts one first image"));
        }
        let (mut route, mut body) = super::wire::generation(model, prompt, options)?;
        if let Some(image) = inputs.first() {
          route = "/v1/image_to_video".into();
          body["promptImage"] = json!(image.uri);
        }
        (route, body, seed)
      }
      Self::Edit { seed, target_aspect_ratio, frames, output_format, prores_profile } => {
        if model != "aleph2" {
          return Err(invalid("Runway editing requires Aleph 2"));
        }
        if frames.len() > 5
          || inputs.len() != frames.len() + 1
          || inputs[0].role != InputRole::SourceVideo
          || inputs[1..].iter().any(|i| i.role != InputRole::ReferenceImage)
        {
          return Err(invalid("Aleph requires one source video and at most five guidance frames"));
        }
        if prompt.chars().count() > 4100 {
          return Err(invalid("Runway edit prompt exceeds Wish's 4100-character limit"));
        }
        if prompt.trim().is_empty() && frames.is_empty() && target_aspect_ratio.is_none() {
          return Err(invalid(
            "Runway edit requires a prompt, guidance frames or target aspect ratio",
          ));
        }
        if options.duration.is_some()
          || options.resolution.is_some()
          || options.aspect_ratio.is_some()
        {
          return Err(invalid(
            "Aleph edit does not accept generation duration/resolution/aspect_ratio",
          ));
        }
        if target_aspect_ratio.as_deref().is_some_and(|r| {
          !["16:9", "4:3", "3:2", "1:1", "2:3", "3:4", "9:16", "21:9"].contains(&r)
        }) {
          return Err(invalid("unsupported Aleph target_aspect_ratio"));
        }
        let ranged = frames.iter().any(|f| f.range.is_some());
        let mut keyframes = Vec::new();
        for (frame, input) in frames.iter().zip(&inputs[1..]) {
          if frame.at.is_some() == frame.seconds.is_some() {
            return Err(invalid("each Aleph keyframe requires exactly one of at or seconds"));
          }
          if frame.at.is_some_and(|n| !n.is_finite() || !(0.0..=1.0).contains(&n))
            || frame.seconds.is_some_and(|n| !n.is_finite() || !(0.0..=30.0).contains(&n))
          {
            return Err(invalid(
              "Aleph at must be in0..1; seconds in0..30, within the actual source duration",
            ));
          }
          if ranged != frame.range.is_some() {
            return Err(invalid("all Aleph keyframes must set an edit range, or none may"));
          }
          let mut value = json!({"uri":input.uri});
          if let Some(at) = frame.at {
            value["at"] = json!(at);
          }
          if let Some(seconds) = frame.seconds {
            value["seconds"] = json!(seconds);
          }
          if let Some(window) = &frame.range {
            if window.start_seconds >= window.end_seconds || window.end_seconds > 30 {
              return Err(invalid(
                "Aleph edit window requires 0 <= start_seconds < end_seconds <=30",
              ));
            }
            if frame.seconds.is_some_and(|n| {
              n < f64::from(window.start_seconds) || n >= f64::from(window.end_seconds)
            }) {
              return Err(invalid(
                "Aleph keyframe seconds must lie inside its edit window (end exclusive)",
              ));
            }
            value["range"] =
              json!({"start_seconds":window.start_seconds,"end_seconds":window.end_seconds});
          }
          keyframes.push(value);
        }
        if let Some(profile) = prores_profile
          && (!matches!(output_format, Some(OutputFormat::Prores))
            || !["422", "4444", "422 Proxy", "422 LT", "422 HQ", "4444 XQ"]
              .contains(&profile.as_str()))
        {
          return Err(invalid(
            "prores_profile requires prores output and a supported ProRes profile",
          ));
        }
        let mut body = json!({"model":model,"videoUri":inputs[0].uri});
        if let Some(format) = output_format {
          body["outputFormat"] = json!(format);
        }
        if let Some(profile) = prores_profile {
          body["proresProfile"] = json!(profile);
        }
        if !prompt.is_empty() {
          body["promptText"] = json!(prompt);
        }
        if !keyframes.is_empty() {
          body["keyframes"] = json!(keyframes);
        }
        if let Some(ratio) = target_aspect_ratio {
          body["targetAspectRatio"] = json!(ratio);
        }
        ("/v1/video_to_video".into(), body, seed)
      }
    };
    if let Some(seed) = seed {
      body["seed"] = json!(seed);
    }
    Ok((route, Draft::post_json(&body)?))
  }
}

impl crate::extensions::media_runtime::NativePlan for Plan {
  fn driver_id(&self) -> &'static str {
    "runway_video"
  }
  fn build(
    &self,
    c: &crate::extensions::media_runtime::PlanContext<'_>,
  ) -> Result<(String, Draft), Error> {
    self.build(
      c.model,
      c.prompt,
      &crate::extensions::shared::parameters::decode(c.parameters)?,
      c.inputs,
    )
  }
  fn artifact_mime(&self) -> Option<&'static str> {
    self.artifact_mime()
  }
}
