//! Supplier-owned media execution contract.
use crate::extensions::media_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
pub struct MinimaxSpeech;
pub static DRIVER: MinimaxSpeech = MinimaxSpeech;
impl MediaDriver for MinimaxSpeech {
  fn validate_subtitles(&self, bytes: &[u8]) -> Result<(), String> {
    super::speech::validate_subtitles(bytes)
  }

  fn id(&self) -> &'static str {
    "minimax_speech"
  }
  fn kind(&self) -> MediaKind {
    MediaKind::Speech
  }
  fn credentials_ready(&self, c: &Credentials) -> bool {
    super::endpoint::ready(c)
  }
  fn call(&self, base: &str, path: &str, c: &Credentials, d: Draft) -> Result<Call, Error> {
    super::endpoint::call(base, path, c, d)
  }
  fn submission(
    &self,
    model: &str,
    prompt: &str,
    p: &Parameters,
  ) -> Result<(String, Draft), Error> {
    super::speech::configured_submission(model, prompt, p, &Default::default())
  }
  fn validate_configuration(&self, model: &str, p: &Parameters) -> Result<(), Error> {
    super::speech::validate_configuration(model, p)
  }
}
