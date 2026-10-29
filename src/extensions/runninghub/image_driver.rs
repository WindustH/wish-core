//! Supplier-owned image execution, preparation and capability policy.
use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct RunninghubImages;
pub static DRIVER: RunninghubImages = RunninghubImages;
impl ImageDriver for RunninghubImages {
  fn id(&self) -> &'static str {
    "runninghub"
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
    crate::extensions::runninghub::image::decode(body)
  }
  fn capabilities(&self, model: &str) -> ImageCapabilities {
    super::image::capabilities(model)
  }
  fn validate(&self, model: &str, parameters: &Parameters) -> Result<(), Error> {
    super::image::validate(model, parameters)
  }
  fn merge_options(
    &self,
    model: &str,
    defaults: Parameters,
    options: &Parameters,
  ) -> Result<Parameters, Error> {
    let values = super::image::merge_options(defaults, options)?;
    self.validate(model, &values)?;
    Ok(values)
  }
  fn task_status(
    &self,
    _model: &str,
    task: &str,
    _parameters: &Parameters,
  ) -> Result<Draft, Error> {
    super::image::status(task)
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
  fn upload<'a>(
    &'a self,
    images: Vec<ReferenceInput>,
    transport: &'a crate::transport::ReqwestTransport,
    base: &'a str,
    credentials: &'a Credentials,
  ) -> Upload<'a> {
    Box::pin(super::image_upload::references(images, transport, base, credentials))
  }
}
