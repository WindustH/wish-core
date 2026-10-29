//! Supplier-owned image execution, preparation and capability policy.
use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct StabilityImages;
pub static DRIVER: StabilityImages = StabilityImages;
impl ImageDriver for StabilityImages {
  fn id(&self) -> &'static str {
    "stability"
  }
  fn credentials_ready(&self, c: &Credentials) -> bool {
    super::endpoint::ready(c)
  }
  fn call(
    &self,
    base: &str,
    path: &str,
    credentials: &Credentials,
    draft: Draft,
  ) -> Result<Call, Error> {
    super::endpoint::call(base, path, credentials, draft)
  }
  fn submission(
    &self,
    _model: &str,
    prompt: &str,
    parameters: &Parameters,
  ) -> Result<Draft, Error> {
    super::image::submission(prompt, parameters)
  }
  fn decode(
    &self,
    body: &Value,
    _current_url: &str,
    _result_url: Option<&str>,
  ) -> Result<ImageReply, Error> {
    crate::extensions::stability::image::decode(body)
  }
}
