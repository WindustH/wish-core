//! Supplier-owned image execution, preparation and capability policy.
use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct OpenaiImages;
pub static DRIVER: OpenaiImages = OpenaiImages;
impl ImageDriver for OpenaiImages {
  fn id(&self) -> &'static str {
    "openai"
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
    crate::extensions::openai::image::decode(body)
  }
  fn capabilities(&self, model: &str) -> ImageCapabilities {
    super::image::capabilities(model)
  }
  fn validate(&self, model: &str, parameters: &Parameters) -> Result<(), Error> {
    super::image::validate_parameters(model, parameters)
  }
  fn with_images(
    &self,
    model: &str,
    prompt: &str,
    parameters: &Parameters,
    images: &[ReferenceInput],
    _references: &[PreciseReference],
    overlay: Option<&EditOverlay>,
  ) -> Result<Draft, Error> {
    if images.is_empty() {
      self.submission(model, prompt, parameters)
    } else {
      super::image::edit(model, prompt, parameters, images, overlay)
    }
  }
  fn prepare(
    &self,
    _model: &str,
    _parameters: &Parameters,
    images: &mut Vec<ReferenceInput>,
    overlay: Option<EditOverlay>,
  ) -> Result<Option<EditOverlay>, String> {
    overlay.map(|overlay| super::preparation::prepare_overlay(images, overlay)).transpose()
  }
}
