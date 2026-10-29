//! Supplier-owned image execution, preparation and capability policy.
use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct SeedreamImages;
pub static DRIVER: SeedreamImages = SeedreamImages;
impl ImageDriver for SeedreamImages {
  fn id(&self) -> &'static str {
    "seedream"
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
  fn submission(&self, model: &str, prompt: &str, parameters: &Parameters) -> Result<Draft, Error> {
    super::image::generation(model, prompt, parameters)
  }
  fn decode(
    &self,
    body: &Value,
    _current_url: &str,
    _result_url: Option<&str>,
  ) -> Result<ImageReply, Error> {
    crate::extensions::seedance::image::decode(body)
  }
  fn capabilities(&self, model: &str) -> ImageCapabilities {
    super::image::capabilities(model)
  }
  fn with_images(
    &self,
    model: &str,
    prompt: &str,
    parameters: &Parameters,
    images: &[ReferenceInput],
    _references: &[PreciseReference],
    _overlay: Option<&EditOverlay>,
  ) -> Result<Draft, Error> {
    if images.is_empty() {
      self.submission(model, prompt, parameters)
    } else {
      super::image::with_images(model, prompt, parameters, images)
    }
  }
}
