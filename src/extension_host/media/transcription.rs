//! Host dispatch retains each extension's original transcription request.
//! This is a closed routing sum, not a shared provider parameter schema.
use crate::extensions::{elevenlabs, openai, siliconflow, xai};
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
pub enum Request {
  Openai(openai::transcribe_tool::Request),
  Siliconflow(siliconflow::transcribe_tool::Request),
  Elevenlabs(elevenlabs::transcribe_tool::Request),
  Xai(xai::Transcribe),
}
impl Request {
  pub fn registration(&self) -> &'static crate::extensions::catalog::Registration {
    match self {
      Self::Openai(_) => &openai::transcribe_tool::REGISTRATION,
      Self::Siliconflow(_) => &siliconflow::transcribe_tool::REGISTRATION,
      Self::Elevenlabs(_) => &elevenlabs::transcribe_tool::REGISTRATION,
      Self::Xai(_) => &xai::TRANSCRIBE,
    }
  }

  pub fn credentials_ready(&self, credentials: &Credentials) -> bool {
    match self {
      Self::Openai(_) => openai::endpoint::ready(credentials),
      Self::Siliconflow(_) => siliconflow::endpoint::ready(credentials),
      Self::Elevenlabs(_) => elevenlabs::endpoint::ready(credentials),
      Self::Xai(_) => xai::endpoint::ready(credentials),
    }
  }
  pub fn call(
    &self,
    base: &str,
    path: &str,
    credentials: &Credentials,
    draft: Draft,
  ) -> Result<Call, Error> {
    match self {
      Self::Openai(_) => openai::endpoint::call(base, path, credentials, draft),
      Self::Siliconflow(_) => siliconflow::endpoint::call(base, path, credentials, draft),
      Self::Elevenlabs(_) => elevenlabs::endpoint::call(base, path, credentials, draft),
      Self::Xai(_) => xai::endpoint::call(base, path, credentials, draft),
    }
  }
  pub fn result(&self, bytes: &[u8]) -> Result<serde_json::Value, String> {
    match self {
      Self::Openai(_) => openai::transcription::result(bytes),
      Self::Siliconflow(_) => siliconflow::transcription::result(bytes),
      Self::Elevenlabs(_) => elevenlabs::transcription::result(bytes),
      Self::Xai(_) => xai::transcription::result(bytes),
    }
  }
  pub fn blob_id(&self) -> &str {
    match self {
      Self::Openai(request) => &request.audio,
      Self::Siliconflow(request) => &request.audio,
      Self::Elevenlabs(request) => &request.audio,
      Self::Xai(request) => &request.audio,
    }
  }
  pub fn service(&self) -> Option<&str> {
    match self {
      Self::Openai(request) => request.service.as_deref(),
      Self::Siliconflow(request) => request.service.as_deref(),
      Self::Elevenlabs(request) => request.service.as_deref(),
      Self::Xai(request) => request.service.as_deref(),
    }
  }
  pub fn validate_audio(&self, byte_len: u64, mime: &str) -> Result<(), Error> {
    match self {
      Self::Openai(request) => request.validate_audio(byte_len, mime),
      Self::Siliconflow(request) => request.validate_audio(byte_len, mime),
      Self::Elevenlabs(request) => request.validate_audio(byte_len, mime),
      Self::Xai(request) => request.validate_audio(byte_len, mime),
    }
  }
  pub fn submission(
    &self,
    model: &str,
    bytes: &[u8],
    mime: &str,
  ) -> Result<(String, Draft), Error> {
    match self {
      Self::Openai(request) => request.audio_submission(model, bytes, mime),
      Self::Siliconflow(request) => request.audio_submission(model, bytes, mime),
      Self::Elevenlabs(request) => request.audio_submission(model, bytes, mime),
      Self::Xai(request) => request.audio_submission(model, bytes, mime),
    }
  }
}
