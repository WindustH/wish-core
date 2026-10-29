//! Supplier-owned media execution contract.
use crate::extensions::media_runtime::*;
use crate::extensions::shared::parameters::decode;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
pub struct ElevenSound;
pub static DRIVER: ElevenSound = ElevenSound;
impl MediaDriver for ElevenSound {
  fn id(&self) -> &'static str {
    "eleven_sound"
  }
  fn kind(&self) -> MediaKind {
    MediaKind::SoundEffect
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
    let (path, body) = super::sound::submission(model, prompt, &decode(p)?)?;
    Ok((path, Draft::post_json(&body)?))
  }
}
