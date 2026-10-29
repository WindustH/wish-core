//! Supplier-owned image execution, preparation and capability policy.
use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct ReplicateImages;
pub static DRIVER: ReplicateImages = ReplicateImages;
impl ImageDriver for ReplicateImages {
  fn id(&self) -> &'static str {
    "replicate"
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
    current_url: &str,
    result_url: Option<&str>,
  ) -> Result<ImageReply, Error> {
    crate::extensions::replicate::image::decode(body, current_url, result_url)
  }
}
