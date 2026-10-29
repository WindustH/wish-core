//! Supplier-owned image execution, preparation and capability policy.
use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct NovelaiImages;
pub static DRIVER: NovelaiImages = NovelaiImages;
impl ImageDriver for NovelaiImages {
  fn validate_configuration(&self, model: &str, parameters: &Parameters) -> Result<(), Error> {
    self.submission(model, "validation", parameters)?;
    self.validate(model, parameters)
  }
  fn id(&self) -> &'static str {
    "novelai"
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
    super::wire::submission(model, prompt, parameters)
  }
  fn decode(
    &self,
    body: &Value,
    _current_url: &str,
    _result_url: Option<&str>,
  ) -> Result<ImageReply, Error> {
    crate::extensions::novelai::wire::decode(body)
  }
  fn capabilities(&self, model: &str) -> ImageCapabilities {
    super::wire::capabilities(model)
  }
  fn validate(&self, model: &str, parameters: &Parameters) -> Result<(), Error> {
    super::characters::validate(model, parameters)
  }
  fn with_images(
    &self,
    model: &str,
    prompt: &str,
    parameters: &Parameters,
    images: &[ReferenceInput],
    references: &[PreciseReference],
    _overlay: Option<&EditOverlay>,
  ) -> Result<Draft, Error> {
    if !references.is_empty() {
      if !self.precise_references(model) || images.len() > 1 {
        return Err(Error::Build(
          "precise references require a NovelAI V4.5 model and at most one source image".into(),
        ));
      }
      let source = match images.first() {
        Some(ReferenceInput::Inline(image)) => Some(image),
        None => None,
        _ => return Err(Error::Build("NovelAI requires a session image blob".into())),
      };
      super::wire::with_references(model, prompt, parameters, source, references)
    } else {
      match images.first() {
        None => self.submission(model, prompt, parameters),
        Some(ReferenceInput::Inline(image)) => super::wire::edit(model, prompt, parameters, image),
        _ => Err(Error::Build("NovelAI requires a session image blob".into())),
      }
    }
  }
  fn prepare_references(
    &self,
    images: Vec<ReferenceInput>,
    settings: Vec<(ReferenceKind, f64, f64)>,
  ) -> Result<Vec<PreciseReference>, String> {
    super::preparation::prepare(images, settings)
  }
  fn precise_references(&self, model: &str) -> bool {
    super::references::supported(model)
  }
}
