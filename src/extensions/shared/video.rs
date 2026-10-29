//! Resolved media assets and descriptive capability metadata.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoMode {
  #[default]
  Text,
  Frames,
  Reference,
  Edit,
  Extend,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRole {
  FirstFrame,
  LastFrame,
  Keyframe,
  ReferenceImage,
  ReferenceVideo,
  ReferenceAudio,
  ReferenceVoice,
  SourceVideo,
}
impl InputRole {
  pub fn media_type(self) -> &'static str {
    match self {
      Self::FirstFrame | Self::LastFrame | Self::Keyframe | Self::ReferenceImage => "image",
      Self::ReferenceAudio | Self::ReferenceVoice => "audio",
      _ => "video",
    }
  }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoInput {
  pub role: InputRole,
  #[serde(default)]
  pub blob_id: Option<String>,
  #[serde(default)]
  pub url: Option<String>,
  #[serde(default)]
  pub job_id: Option<String>,
  #[serde(default)]
  pub timestamp_s: Option<f64>,
  #[serde(default)]
  pub voice_id: Option<String>,
}
pub struct ResolvedInput {
  pub role: InputRole,
  pub uri: String,
  pub timestamp_s: Option<f64>,
  pub voice_id: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct VideoCapabilities {
  pub modes: Vec<VideoMode>,
  pub input_roles: Vec<InputRole>,
  pub blob_inputs: Vec<String>,
  pub notes: String,
}
/// Provider-specific status fields stay in the wire codec, outside job persistence.
pub enum VideoStatus {
  Pending,
  Complete(String),
  Failed,
}

/// Validate an opaque task identifier before inserting it in a polling URL.
pub fn task_identifier(value: &serde_json::Value) -> Result<String, String> {
  value
    .as_str()
    .filter(|s| {
      !s.is_empty()
        && s.len() <= 200
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
    .map(str::to_owned)
    .ok_or_else(|| "submission returned no valid task ID; outcome unknown, do not resubmit".into())
}

/// Shared asset-shape validation. Suppliers select their capabilities and whether task references are valid.
pub fn validate_sources(
  caps: &VideoCapabilities,
  mode: VideoMode,
  inputs: &[VideoInput],
  allow_job: bool,
) -> Result<(), crate::protocol::error::Error> {
  use InputRole::*;
  use VideoMode::*;
  let invalid = |text: &str| crate::protocol::error::Error::Build(text.into());
  if !caps.modes.contains(&mode) {
    return Err(invalid(
      "selected model does not support this video mode; inspect the extension tool",
    ));
  }
  if inputs.len() > 50 {
    return Err(invalid("too many video inputs"));
  }
  for input in inputs {
    if (input.role == Keyframe) != input.timestamp_s.is_some() {
      return Err(invalid("timestamp_s is required only for keyframe inputs"));
    }
    if !caps.input_roles.contains(&input.role) {
      return Err(invalid("input role is not supported by selected model"));
    }
    if (input.role == ReferenceVoice) != input.voice_id.is_some() {
      return Err(invalid("voice_id is required only for reference_voice inputs"));
    }
    if let Some(voice) = &input.voice_id
      && (voice.is_empty()
        || voice.len() > 100
        || !voice.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    {
      return Err(invalid("invalid preset voice_id"));
    }
    if usize::from(input.voice_id.is_some())
      + usize::from(input.blob_id.is_some())
      + usize::from(input.url.is_some())
      + usize::from(input.job_id.is_some())
      != 1
    {
      return Err(invalid(
        "each input requires exactly one source: blob_id, url, job_id, or preset voice_id",
      ));
    }
    if input.job_id.is_some() && !(allow_job && mode == Extend && input.role == SourceVideo) {
      return Err(invalid("job_id is not supported for this input"));
    }
  }
  let count = |role| inputs.iter().filter(|i| i.role == role).count();
  if count(FirstFrame) > 1 || count(LastFrame) > 1 || count(SourceVideo) > 1 {
    return Err(invalid("first_frame, last_frame and source_video must be unique"));
  }
  Ok(())
}
