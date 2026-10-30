//! Supplier-owned image execution, preparation and capability policy.
use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct AntigravityImages;
pub static DRIVER: AntigravityImages = AntigravityImages;
impl ImageDriver for AntigravityImages {
  fn id(&self) -> &'static str {
    "google_antigravity_image"
  }
  fn credentials_ready(&self, c: &Credentials) -> bool {
    !c.api_key.is_empty() && c.project.as_deref().is_some_and(|p| !p.is_empty())
  }
  fn call(
    &self,
    base: &str,
    path: &str,
    credentials: &Credentials,
    draft: Draft,
  ) -> Result<Call, Error> {
    let body: Value = serde_json::from_slice(&draft.body)
      .map_err(|e| Error::Build(format!("invalid image draft: {e}")))?;
    let model =
      body["model"].as_str().ok_or_else(|| Error::Build("image draft has no model".into()))?;
    let draft =
      super::antigravity::draft(model, body["request"].clone(), "image_gen", credentials)?;
    super::antigravity::source(self.id()).to_endpoint(Some(base), Some(path))?.build_call(
      draft,
      credentials,
      crate::utils::time::unix_seconds(),
    )
  }
  fn submission(&self, model: &str, prompt: &str, parameters: &Parameters) -> Result<Draft, Error> {
    submission(model, prompt, parameters, &[])
  }
  fn decode(
    &self,
    body: &Value,
    _current_url: &str,
    _result_url: Option<&str>,
  ) -> Result<ImageReply, Error> {
    super::image::decode(super::antigravity::response(body)?)
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
  fn with_images(
    &self,
    model: &str,
    prompt: &str,
    parameters: &Parameters,
    images: &[ReferenceInput],
    _references: &[PreciseReference],
    _overlay: Option<&EditOverlay>,
  ) -> Result<Draft, Error> {
    submission(model, prompt, parameters, images)
  }
}

fn submission(
  model: &str,
  prompt: &str,
  parameters: &Parameters,
  images: &[ReferenceInput],
) -> Result<Draft, Error> {
  let draft = super::image::submission(model, prompt, parameters, images)?;
  let request: Value = serde_json::from_slice(&draft.body)
    .map_err(|e| Error::Build(format!("invalid image draft: {e}")))?;
  Draft::post_json(&serde_json::json!({"model": model, "request": request}))
}
