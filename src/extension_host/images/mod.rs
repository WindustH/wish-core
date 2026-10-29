//! Registry-backed image runner. All supplier decisions belong to registered drivers.
pub(crate) mod execution;
pub use crate::extensions::image_runtime::*;
use crate::protocol::{
  attempt::{Call, Method},
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
#[derive(Clone, Copy)]
pub struct ImageProtocol(pub &'static dyn ImageDriver);
impl ImageProtocol {
  pub fn max_output_images(self) -> u64 {
    self.0.max_output_images()
  }
  pub fn subscription(self) -> bool {
    self.0.subscription()
  }
  pub fn capabilities(self, model: &str) -> ImageCapabilities {
    self.0.capabilities(model)
  }
  pub fn credentials_ready(self, c: &Credentials) -> bool {
    self.0.credentials_ready(c)
  }
  pub fn call(self, base: &str, path: &str, c: &Credentials, d: Draft) -> Result<Call, Error> {
    self.0.call(base, path, c, d)
  }
  pub fn decode(self, body: &Value, url: &str, result: Option<&str>) -> Result<ImageReply, Error> {
    self.0.decode(body, url, result)
  }
  pub fn task_status(self, model: &str, task: &str, p: &Parameters) -> Result<Draft, Error> {
    self.0.task_status(model, task, p)
  }
  pub fn request_options(self, model: &str) -> &'static [&'static str] {
    self.capabilities(model).options
  }
  pub fn accepts_options(self, model: &str, p: &Parameters) -> bool {
    p.keys().all(|key| self.request_options(model).contains(&key.as_str()))
  }
  pub fn merge_options(
    self,
    model: &str,
    defaults: Parameters,
    p: &Parameters,
  ) -> Result<Parameters, Error> {
    if !self.accepts_options(model, p) {
      return Err(Error::Build("unsupported image parameters".into()));
    }
    self.0.merge_options(model, defaults, p)
  }
  pub fn accepts_input(self, model: &str, value: &str) -> bool {
    let c = self.capabilities(model);
    c.reference_limit > 0
      && if value.starts_with("https://") { c.remote_references } else { c.local_references }
  }
  pub fn reference_images(self, model: &str) -> bool {
    self.reference_image_limit(model) > 0
  }
  pub fn reference_image_limit(self, model: &str) -> usize {
    self.capabilities(model).reference_limit
  }
  pub fn accepts_reference_count(self, model: &str, n: usize) -> bool {
    n <= self.reference_image_limit(model) && (!self.image_required(model) || n == 1)
  }
  pub fn mask_editing(self, model: &str) -> bool {
    let c = self.capabilities(model);
    c.reference_limit > 0 && c.mask
  }
  pub fn markup_editing(self, model: &str) -> bool {
    self.capabilities(model).markup
  }
  pub fn image_required(self, model: &str) -> bool {
    self.capabilities(model).image_required
  }
  pub fn precise_references(self, model: &str) -> bool {
    self.0.precise_references(model)
  }
  pub fn submission_with_images(
    self,
    model: &str,
    prompt: &str,
    p: &Parameters,
    images: &[ReferenceInput],
    references: &[PreciseReference],
    overlay: Option<&EditOverlay>,
  ) -> Result<Draft, Error> {
    if overlay.is_some_and(|v| match v {
      EditOverlay::Mask(_) => !self.mask_editing(model),
      EditOverlay::Markup(_) => !self.markup_editing(model),
    }) {
      return Err(Error::Build("unsupported image overlay".into()));
    }
    if !self.accepts_reference_count(model, images.len()) {
      return Err(Error::Build("unsupported reference image count".into()));
    }
    self.0.with_images(model, prompt, p, images, references, overlay)
  }
}
impl std::fmt::Debug for ImageProtocol {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.0.id())
  }
}
impl PartialEq for ImageProtocol {
  fn eq(&self, o: &Self) -> bool {
    self.0.id() == o.0.id()
  }
}
impl Eq for ImageProtocol {}
impl serde::Serialize for ImageProtocol {
  fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(self.0.id())
  }
}
impl<'de> serde::Deserialize<'de> for ImageProtocol {
  fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
    let id = <String as serde::Deserialize>::deserialize(d)?;
    crate::extensions::image_registry::drivers()
      .iter()
      .find(|driver| driver.id() == id)
      .map(|driver| Self(*driver))
      .ok_or_else(|| serde::de::Error::custom(format!("unknown image implementation: {id}")))
  }
}
pub fn poll_call(protocol: ImageProtocol, initial: &Call, url: &str) -> Result<Call, Error> {
  let origin = crate::extensions::shared::urls::validate_url(&initial.url)?;
  let target = crate::extensions::shared::urls::validate_url(url)?;
  if !protocol.0.permits_poll_origin(&origin, &target) {
    return Err(Error::Build("image polling URL has an untrusted origin".into()));
  }
  Ok(Call {
    method: Method::Get,
    url: target.into(),
    headers: initial
      .headers
      .iter()
      .filter(|(key, _)| !key.eq_ignore_ascii_case("content-type"))
      .cloned()
      .collect(),
    body: Vec::new(),
  })
}
