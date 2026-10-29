//! xAI owns its transcription tool parameters; generic host code only supplies owned blobs/services.
use crate::{
  extensions::catalog::{Invocation, Permission, Registration},
  protocol::Tool,
};
use serde::Deserialize;
use serde_json::json;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transcribe {
  pub audio: String,
  #[serde(default)]
  pub service: Option<String>,
  #[serde(default)]
  pub language: Option<String>,
  #[serde(default)]
  pub diarize: bool,
  #[serde(default)]
  pub format: Option<bool>,
  #[serde(default)]
  pub multichannel: Option<bool>,
  #[serde(default)]
  pub filler_words: Option<bool>,
  #[serde(default)]
  pub vad_threshold: Option<f64>,
  #[serde(default)]
  pub keyterms: Option<Vec<String>>,
}
pub fn specification() -> Tool {
  Tool {
    name:"xai_transcribe".into(),
    description:"Transcribe a session-owned audio/video blob using xAI. audio is its 64-character blob ID, not a local path or public URL. Wish accepts MP3/WAV/FLAC/Ogg/MP4/WebM containers up to 32 MiB; raw/headerless audio and live streaming are not supported by this tool. Omit service to choose the first compatible configured xAI transcription service. language is optional; format=true normalizes spoken numbers/currency and requires a supported language. diarize adds speaker IDs; multichannel separates container channels. keyterms boosts up to 100 terms of 1–50 characters. vad_threshold is 0..1, with 0 disabling the speech gate. Returns transcript text and a local JSON blob containing complete word timestamps, speakers and channel data when available. Interrupted paid requests are never automatically repeated or sent to another service.".into(),
    input_schema:json!({"type":"object","additionalProperties":false,"required":["audio"],"properties":{
      "audio":{"type":"string","pattern":"^[a-f0-9]{64}$"},
      "service":{"type":"string","description":"Optional configured xAI ASR service ID; default follows configuration priority."},
      "language":{"type":"string"},"diarize":{"type":"boolean"},
      "format":{"type":"boolean"},"multichannel":{"type":"boolean"},"filler_words":{"type":"boolean"},
      "vad_threshold":{"type":"number","minimum":0,"maximum":1},
      "keyterms":{"type":"array","maxItems":100,"items":{"type":"string","minLength":1,"maxLength":50}}
    }})
  }
}
pub const TRANSCRIBE: Registration = Registration {
  extension: "xai",
  name: "xai_transcribe",
  summary: "xAI file transcription with speakers, timestamps, channels and terminology hints",
  tags: &["audio", "asr", "transcription", "speech-to-text"],
  service_presets: &["xai_asr"],
  accepts_model: |_| true,
  permission: Permission::Transcription,
  specification,
  examples: || json!([{"audio":"<session audio blob ID>","language":"en","diarize":true,"format":true,"keyterms":["Wish"]}]),
  parse: |value| {
    serde_json::from_value::<Transcribe>(value).map(Invocation::new).map_err(|e| e.to_string())
  },
};

pub mod speech;

pub mod video;

pub mod endpoint;
pub(crate) mod image;
pub(crate) mod image_tool;
pub(crate) mod speech_output;
pub(crate) mod speech_wire;
pub(crate) mod transcription;
pub mod video_contract;
pub mod video_options;
pub mod video_status;
pub(crate) mod video_wire;

impl Transcribe {
  pub(crate) fn validate_audio(
    &self,
    byte_len: u64,
    mime: &str,
  ) -> Result<(), crate::protocol::error::Error> {
    transcription::validate(
      byte_len,
      mime,
      self.language.as_deref(),
      self.diarize,
      &self.transcription_options(),
    )
    .map(|_| ())
  }
  pub(crate) fn audio_submission(
    &self,
    model: &str,
    bytes: &[u8],
    mime: &str,
  ) -> Result<(String, crate::protocol::endpoint::Draft), crate::protocol::error::Error> {
    transcription::submission(
      model,
      bytes,
      mime,
      self.language.as_deref(),
      self.diarize,
      &self.transcription_options(),
    )
  }
  fn transcription_options(&self) -> transcription::Options {
    transcription::Options {
      format: self.format,
      multichannel: self.multichannel,
      filler_words: self.filler_words,
      vad_threshold: self.vad_threshold,
      keyterms: self.keyterms.clone().unwrap_or_default(),
    }
  }
}

pub(crate) mod video_operation;

pub(crate) mod image_driver;

pub(crate) mod xai_video_driver;

pub(crate) mod xai_speech_driver;

pub(crate) mod xai_transcription_driver;

pub(crate) mod transcription_driver;

pub(crate) mod execution;

pub(crate) mod package;
