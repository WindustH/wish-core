//! Supplier-owned media execution contract.
use crate::extensions::media_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
pub struct OpenaiTranscription;
pub static DRIVER: OpenaiTranscription = OpenaiTranscription;
impl MediaDriver for OpenaiTranscription {
  fn id(&self) -> &'static str {
    "openai_transcription"
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
    Err(Error::Build("use this extension transcription tool".into()))
  }
  fn validate_configuration(&self, model: &str, p: &Parameters) -> Result<(), Error> {
    super::transcription::validate_configuration(model, p)
  }
}
