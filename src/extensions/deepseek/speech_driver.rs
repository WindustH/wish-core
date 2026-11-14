use crate::extensions::{media_runtime::*, shared::parameters::decode};
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};

pub struct DeepseekSpeech;
pub static DRIVER: DeepseekSpeech = DeepseekSpeech;

impl MediaDriver for DeepseekSpeech {
  fn id(&self) -> &'static str {
    "deepseek_speech"
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
  fn submission(&self, _model: &str, text: &str, p: &Parameters) -> Result<(String, Draft), Error> {
    super::speech::submission(text, &decode(p)?)
  }
  fn validate_configuration(&self, model: &str, p: &Parameters) -> Result<(), Error> {
    if model != "tts" {
      return Err(Error::Build("DeepSeek speech has no selectable model; use tts".into()));
    }
    self.submission(model, "validation", p).map(|_| ())
  }
}
