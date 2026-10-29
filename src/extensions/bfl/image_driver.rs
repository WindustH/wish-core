//! Supplier-owned image execution, preparation and capability policy.
use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct BflImages;
pub static DRIVER: BflImages = BflImages;
impl ImageDriver for BflImages {
  fn id(&self) -> &'static str {
    "bfl"
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
    crate::extensions::bfl::image::decode(body, current_url, result_url)
  }
  fn capabilities(&self, model: &str) -> ImageCapabilities {
    super::image::capabilities(model)
  }
  fn validate(&self, model: &str, parameters: &Parameters) -> Result<(), Error> {
    if parameters.keys().any(|key| key.starts_with("input_image")) {
      return Err(Error::Build("BFL input images are managed by the tool images argument".into()));
    }
    super::image::validate(model, parameters)
  }
  fn merge_options(
    &self,
    model: &str,
    defaults: Parameters,
    options: &Parameters,
  ) -> Result<Parameters, Error> {
    let values = super::image::merge_options(model, defaults, options)?;
    self.validate(model, &values)?;
    Ok(values)
  }
  fn permits_poll_origin(&self, origin: &reqwest::Url, target: &reqwest::Url) -> bool {
    origin.origin() == target.origin() || super::endpoint::permits_poll_origin(origin, target)
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
    if self.capabilities(model).image_required {
      if !references.is_empty() {
        return Err(Error::Build("Fill does not accept precise references".into()));
      }
      super::image::fill(prompt, parameters, images, overlay.map(EditOverlay::image))
    } else if images.is_empty() {
      self.submission(model, prompt, parameters)
    } else {
      super::image::edit(model, prompt, parameters, images)
    }
  }
  fn prepare(
    &self,
    model: &str,
    _parameters: &Parameters,
    images: &mut Vec<ReferenceInput>,
    overlay: Option<EditOverlay>,
  ) -> Result<Option<EditOverlay>, String> {
    if self.capabilities(model).image_required {
      super::preparation::validate(images, overlay.as_ref().map(EditOverlay::image))?;
    }
    Ok(overlay)
  }
}
