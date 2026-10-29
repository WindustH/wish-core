//! Optional image runner contract. Each supplier opts in and owns all format decisions.

pub use super::shared::images::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use crate::transport::ReqwestTransport;
use serde_json::{Map, Value};
use std::{future::Future, pin::Pin};
pub type Parameters = Map<String, Value>;
pub type Upload<'a> =
  Pin<Box<dyn Future<Output = Result<Vec<ReferenceInput>, String>> + Send + 'a>>;
pub trait ImageDriver: Sync {
  fn id(&self) -> &'static str;
  fn subscription(&self) -> bool {
    false
  }
  fn max_output_images(&self) -> u64 {
    8
  }
  fn capabilities(&self, _model: &str) -> ImageCapabilities {
    Default::default()
  }
  fn credentials_ready(&self, credentials: &Credentials) -> bool;
  fn call(
    &self,
    base: &str,
    path: &str,
    credentials: &Credentials,
    draft: Draft,
  ) -> Result<Call, Error>;
  fn submission(&self, model: &str, prompt: &str, parameters: &Parameters) -> Result<Draft, Error>;
  fn decode(
    &self,
    body: &Value,
    current_url: &str,
    result_url: Option<&str>,
  ) -> Result<ImageReply, Error>;
  fn task_status(
    &self,
    _model: &str,
    _task: &str,
    _parameters: &Parameters,
  ) -> Result<Draft, Error> {
    Err(Error::Build("extension has no image task endpoint".into()))
  }
  fn permits_poll_origin(&self, origin: &reqwest::Url, target: &reqwest::Url) -> bool {
    origin.origin() == target.origin()
  }
  fn validate_configuration(&self, model: &str, parameters: &Parameters) -> Result<(), Error> {
    self.validate(model, parameters)
  }
  fn validate(&self, _model: &str, _parameters: &Parameters) -> Result<(), Error> {
    Ok(())
  }
  fn merge_options(
    &self,
    model: &str,
    mut defaults: Parameters,
    options: &Parameters,
  ) -> Result<Parameters, Error> {
    defaults.extend(options.clone());
    self.validate(model, &defaults)?;
    Ok(defaults)
  }
  fn prepare_references(
    &self,
    _images: Vec<ReferenceInput>,
    _settings: Vec<(ReferenceKind, f64, f64)>,
  ) -> Result<Vec<PreciseReference>, String> {
    Err("extension does not support precise references".into())
  }
  fn precise_references(&self, _model: &str) -> bool {
    false
  }
  fn with_images(
    &self,
    model: &str,
    prompt: &str,
    parameters: &Parameters,
    images: &[ReferenceInput],
    references: &[PreciseReference],
    overlay: Option<&EditOverlay>,
  ) -> Result<Draft, Error> {
    if !images.is_empty() || !references.is_empty() || overlay.is_some() {
      return Err(Error::Build("extension does not support these image inputs".into()));
    }
    self.submission(model, prompt, parameters)
  }
  fn prepare(
    &self,
    _model: &str,
    _parameters: &Parameters,
    _images: &mut Vec<ReferenceInput>,
    overlay: Option<EditOverlay>,
  ) -> Result<Option<EditOverlay>, String> {
    Ok(overlay)
  }
  fn upload<'a>(
    &'a self,
    images: Vec<ReferenceInput>,
    _transport: &'a ReqwestTransport,
    _base: &'a str,
    _credentials: &'a Credentials,
  ) -> Upload<'a> {
    Box::pin(async move { Ok(images) })
  }
}

use serde::Deserialize;
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
  Character,
  Style,
  CharacterStyle,
}
impl ReferenceKind {
  pub fn caption(self) -> &'static str {
    match self {
      Self::Character => "character",
      Self::Style => "style",
      Self::CharacterStyle => "character&style",
    }
  }
}
pub struct PreciseReference {
  pub image: ReferenceImage,
  pub kind: ReferenceKind,
  pub strength: f64,
  pub fidelity: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRequest {
  pub prompt: String,
  #[serde(default)]
  pub options: serde_json::Map<String, Value>,
  #[serde(default)]
  pub images: Vec<String>,
  #[serde(default)]
  pub mask: Option<String>,
  #[serde(default)]
  pub markup: Option<String>,
  #[serde(default)]
  pub references: Vec<PreciseReferenceRequest>,
  #[serde(default)]
  pub provider: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreciseReferenceRequest {
  pub image: String,
  pub kind: ReferenceKind,
  #[serde(default = "reference_default")]
  pub strength: f64,
  #[serde(default = "reference_default")]
  pub fidelity: f64,
}
fn reference_default() -> f64 {
  1.0
}
