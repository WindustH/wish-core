//! Supplier-owned image execution, preparation and capability policy.
use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct WanImages;
pub static DRIVER: WanImages = WanImages;
impl ImageDriver for WanImages {
  fn id(&self) -> &'static str {
    "wan"
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
    super::image::submission(model, prompt, parameters, &[])
  }
  fn decode(
    &self,
    body: &Value,
    _current_url: &str,
    _result_url: Option<&str>,
  ) -> Result<ImageReply, Error> {
    crate::extensions::alibaba::image::decode(body)
  }
  fn max_output_images(&self) -> u64 {
    12
  }
  fn capabilities(&self, model: &str) -> ImageCapabilities {
    super::image::capabilities(model)
  }
  fn validate(&self, model: &str, parameters: &Parameters) -> Result<(), Error> {
    super::image::validate(model, parameters, false)
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
    super::image::submission(model, prompt, parameters, images)
  }
  fn prepare(
    &self,
    _model: &str,
    parameters: &Parameters,
    images: &mut Vec<ReferenceInput>,
    overlay: Option<EditOverlay>,
  ) -> Result<Option<EditOverlay>, String> {
    super::preparation::prepare_references(images, parameters)?;
    Ok(overlay)
  }
}
