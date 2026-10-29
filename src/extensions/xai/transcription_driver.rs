//! This extension's typed transcription execution.
use crate::extensions::transcription_runtime::Transcription;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
impl Transcription for super::Transcribe {
  fn registration(&self) -> &'static crate::extensions::catalog::Registration {
    &super::TRANSCRIBE
  }
  fn credentials_ready(&self, c: &Credentials) -> bool {
    super::endpoint::ready(c)
  }
  fn call(&self, base: &str, path: &str, c: &Credentials, d: Draft) -> Result<Call, Error> {
    super::endpoint::call(base, path, c, d)
  }
  fn result(&self, bytes: &[u8]) -> Result<serde_json::Value, String> {
    super::transcription::result(bytes)
  }
  fn blob_id(&self) -> &str {
    &self.audio
  }
  fn service(&self) -> Option<&str> {
    self.service.as_deref()
  }
  fn validate_audio(&self, byte_len: u64, mime: &str) -> Result<(), Error> {
    self.validate_audio(byte_len, mime)
  }
  fn submission(&self, model: &str, bytes: &[u8], mime: &str) -> Result<(String, Draft), Error> {
    self.audio_submission(model, bytes, mime)
  }
}

impl crate::extensions::invocation::Executable for super::Transcribe {
  fn execute<'a>(
    self: Box<Self>,
    host: &'a dyn crate::extensions::invocation::Host,
  ) -> crate::extensions::invocation::Future<'a> {
    Box::pin(async move { host.transcribe(self).await })
  }
}
