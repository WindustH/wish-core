use crate::extensions::media_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};

pub struct DeepseekTranscription;
pub static DRIVER: DeepseekTranscription = DeepseekTranscription;

impl MediaDriver for DeepseekTranscription {
  fn id(&self) -> &'static str {
    "deepseek_transcription"
  }
  fn kind(&self) -> MediaKind {
    MediaKind::Transcription
  }
  fn credentials_ready(&self, c: &Credentials) -> bool {
    super::endpoint::ready(c)
  }
  fn call(&self, base: &str, path: &str, c: &Credentials, d: Draft) -> Result<Call, Error> {
    super::endpoint::call(base, path, c, d)
  }
  fn submission(
    &self,
    _model: &str,
    _prompt: &str,
    _p: &Parameters,
  ) -> Result<(String, Draft), Error> {
    Err(Error::Build("use deepseek_transcribe with an audio blob".into()))
  }
  fn validate_configuration(&self, model: &str, p: &Parameters) -> Result<(), Error> {
    if model != "asr" {
      return Err(Error::Build("DeepSeek ASR has no selectable model; use asr".into()));
    }
    if p.values().any(|v| !v.is_null()) {
      return Err(Error::Build("DeepSeek ASR does not accept transcription defaults".into()));
    }
    Ok(())
  }
}
