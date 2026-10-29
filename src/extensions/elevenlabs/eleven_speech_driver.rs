//! Supplier-owned media execution contract.
use crate::extensions::media_runtime::*;
use crate::extensions::shared::parameters::decode;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
pub struct ElevenSpeech;
pub static DRIVER: ElevenSpeech = ElevenSpeech;
impl MediaDriver for ElevenSpeech {
  fn id(&self) -> &'static str {
    "eleven_speech"
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
    let (path, body) = super::speech::submission(model, prompt, &decode(p)?)?;
    Ok((path, Draft::post_json(&body)?))
  }
  fn validate_configuration(&self, model: &str, p: &Parameters) -> Result<(), Error> {
    super::speech::validate_configuration(model, p)
  }
}
