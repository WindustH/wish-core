//! Supplier-owned image execution, preparation and capability policy.
use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct CodexImages;
pub static DRIVER: CodexImages = CodexImages;
impl ImageDriver for CodexImages {
  fn id(&self) -> &'static str {
    "codex"
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
    super::endpoint::subscription_image(base, path, credentials, draft)
  }
  fn submission(&self, model: &str, prompt: &str, parameters: &Parameters) -> Result<Draft, Error> {
    super::subscription_image::generation(model, prompt, parameters)
  }
  fn decode(
    &self,
    body: &Value,
    _current_url: &str,
    _result_url: Option<&str>,
  ) -> Result<ImageReply, Error> {
    crate::extensions::openai::subscription_image::decode(body)
  }
  fn subscription(&self) -> bool {
    true
  }
}
