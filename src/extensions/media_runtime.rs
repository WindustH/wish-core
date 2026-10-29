//! Optional durable-file runner ports. Suppliers own parameters, input rules and wire formats.
pub use super::shared::parameters::Parameters;
pub use super::shared::video::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use crate::transport::ReqwestTransport;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
  Video,
  Speech,
  Music,
  SoundEffect,
  Transcription,
}
impl MediaKind {
  pub fn as_str(self) -> &'static str {
    match self {
      Self::Video => "video",
      Self::Speech => "speech",
      Self::Music => "music",
      Self::SoundEffect => "sound_effect",
      Self::Transcription => "transcription",
    }
  }
  pub fn file_kind(self) -> &'static str {
    if self == Self::Video { "video/" } else { "audio/" }
  }
}

pub type Upload<'a> = Pin<Box<dyn Future<Output = Result<Option<String>, String>> + Send + 'a>>;
pub trait MediaDriver: Sync {
  fn buffered_audio(&self, _p: &Parameters) -> bool {
    false
  }
  fn decode_audio(&self, _bytes: &[u8]) -> Result<AudioCompletion, String> {
    Err("extension does not decode buffered audio".into())
  }
  fn validate_subtitles(&self, _bytes: &[u8]) -> Result<(), String> {
    Err("extension does not accept subtitles".into())
  }
  fn artifact_validator(&self, _mime: Option<&str>) -> Option<crate::storage::media::Validator> {
    None
  }

  fn id(&self) -> &'static str;
  fn kind(&self) -> MediaKind;
  fn credentials_ready(&self, c: &Credentials) -> bool;
  fn call(&self, base: &str, path: &str, c: &Credentials, d: Draft) -> Result<Call, Error>;
  fn submission(&self, model: &str, prompt: &str, p: &Parameters)
  -> Result<(String, Draft), Error>;
  fn validate_configuration(&self, model: &str, p: &Parameters) -> Result<(), Error> {
    self.submission(model, "validation", p).map(|_| ())
  }
  fn parameters(
    &self,
    defaults: &Parameters,
    overrides: &Parameters,
    _mode: VideoMode,
    _inputs: &[VideoInput],
  ) -> Result<Parameters, Error> {
    Ok(super::shared::parameters::overlay(defaults, overrides))
  }
  fn task_id(&self, _v: &Value) -> Result<String, String> {
    Err("extension does not return a video task".into())
  }
  fn status_request(&self, _task: &str) -> Result<(String, Draft), Error> {
    Err(Error::Build("extension has no video status API".into()))
  }
  fn decode_status(&self, _v: &Value) -> Result<VideoStatus, Error> {
    Err(Error::Build("extension has no video status API".into()))
  }
  fn video_capabilities(&self, _model: &str) -> Option<VideoCapabilities> {
    None
  }
  fn validate_video_inputs(
    &self,
    _model: &str,
    _mode: VideoMode,
    _inputs: &[VideoInput],
  ) -> Result<(), Error> {
    Err(Error::Build("extension does not support video".into()))
  }
  fn video_submission(
    &self,
    _model: &str,
    _prompt: &str,
    _mode: VideoMode,
    _p: &Parameters,
    _inputs: &[ResolvedInput],
    _modify: Option<&str>,
  ) -> Result<(String, Draft), Error> {
    Err(Error::Build("extension does not support video".into()))
  }
  fn validate_input_url(&self, value: &str) -> Result<(), String> {
    super::shared::urls::validate_asset_url(value, None)
  }
  fn requires_upload(&self, _bytes: usize) -> bool {
    false
  }
  fn upload<'a>(
    &'a self,
    _bytes: Vec<u8>,
    _mime: &'a str,
    _base: &'a str,
    _c: &'a Credentials,
    _transport: &'a ReqwestTransport,
    _status_transport: &'a ReqwestTransport,
  ) -> Upload<'a> {
    Box::pin(async { Ok(None) })
  }
}

/// A supplier may carry a native plan instead of adopting the optional parameter helper.
pub struct PlanContext<'a> {
  pub model: &'a str,
  pub prompt: &'a str,
  pub defaults: &'a Parameters,
  pub parameters: &'a Parameters,
  pub inputs: &'a [ResolvedInput],
}
pub enum AudioCompletion {
  Inline { audio: Vec<u8>, metadata: Vec<u8> },
  Download { audio: String, subtitles: Option<String> },
}
pub trait NativePlan: Send + Sync {
  fn driver_id(&self) -> &'static str;
  fn build(&self, context: &PlanContext<'_>) -> Result<(String, Draft), Error>;
  fn artifact_mime(&self) -> Option<&'static str> {
    None
  }
  fn buffered_audio(&self) -> bool {
    false
  }
  fn decode_audio(&self, _bytes: &[u8]) -> Result<AudioCompletion, String> {
    Err("plan does not decode audio".into())
  }
}

#[derive(Default)]
pub struct MediaRequest {
  pub plan: Option<std::sync::Arc<dyn crate::extensions::media_runtime::NativePlan>>,
  pub action: Option<String>,
  pub provider: Option<String>,
  pub prompt: Option<String>,
  pub kind: Option<MediaKind>,
  pub job_id: Option<String>,
  pub options: Parameters,
  pub mode: Option<VideoMode>,
  pub inputs: Vec<VideoInput>,
  pub modify_mode: Option<String>,
}
