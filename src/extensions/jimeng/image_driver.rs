//! Supplier-owned image execution, preparation and capability policy.
use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct JimengImages;
pub static DRIVER: JimengImages = JimengImages;
impl ImageDriver for JimengImages {
  fn validate_configuration(&self, model: &str, parameters: &Parameters) -> Result<(), Error> {
    self.submission(model, "validation", parameters)?;
    self.validate(model, parameters)
  }
  fn id(&self) -> &'static str {
    "jimeng"
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
    crate::extensions::jimeng::image::decode(body)
  }
  fn capabilities(&self, model: &str) -> ImageCapabilities {
    super::image::capabilities(model)
  }
  fn task_status(&self, model: &str, task: &str, parameters: &Parameters) -> Result<Draft, Error> {
    super::image::status_request(model, task, parameters)
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
}
