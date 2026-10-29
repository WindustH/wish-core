//! Registry-backed optional media runner. No supplier wire dispatch.
pub mod transcription;
pub mod video;
pub use crate::extensions::media_runtime::{MediaDriver, MediaKind, Parameters, VideoStatus};
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
#[derive(Clone, Copy)]
pub struct MediaProtocol(pub &'static dyn MediaDriver);
impl MediaProtocol {
  pub fn kind(self) -> MediaKind {
    self.0.kind()
  }
  pub fn credentials_ready(self, c: &Credentials) -> bool {
    self.0.credentials_ready(c)
  }
  pub fn call(self, base: &str, path: &str, c: &Credentials, d: Draft) -> Result<Call, Error> {
    self.0.call(base, path, c, d)
  }
  pub fn submission(
    self,
    model: &str,
    prompt: &str,
    p: &Parameters,
  ) -> Result<(String, Draft), Error> {
    self.0.submission(model, prompt, p)
  }
  pub fn validate_configuration(self, model: &str, p: &Parameters) -> Result<(), Error> {
    self.0.validate_configuration(model, p)
  }
  pub fn task_id(self, v: &Value) -> Result<String, String> {
    self.0.task_id(v)
  }
  pub fn status_request(self, task: &str) -> Result<(String, Draft), Error> {
    self.0.status_request(task)
  }
  pub fn decode_status(self, v: &Value) -> Result<VideoStatus, Error> {
    self.0.decode_status(v)
  }
  pub fn parameters(
    self,
    d: &Parameters,
    o: &Parameters,
    mode: video::VideoMode,
    inputs: &[video::VideoInput],
  ) -> Result<Parameters, Error> {
    self.0.parameters(d, o, mode, inputs)
  }
  pub fn video_capabilities(self, model: &str) -> Option<video::VideoCapabilities> {
    self.0.video_capabilities(model)
  }
  pub fn validate_video_inputs(
    self,
    model: &str,
    mode: video::VideoMode,
    inputs: &[video::VideoInput],
  ) -> Result<(), Error> {
    self.0.validate_video_inputs(model, mode, inputs)
  }
  pub fn video_submission(
    self,
    model: &str,
    prompt: &str,
    mode: video::VideoMode,
    p: &Parameters,
    inputs: &[video::ResolvedInput],
    modify: Option<&str>,
  ) -> Result<(String, Draft), Error> {
    self.0.video_submission(model, prompt, mode, p, inputs, modify)
  }
}
impl std::fmt::Debug for MediaProtocol {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.0.id())
  }
}
impl PartialEq for MediaProtocol {
  fn eq(&self, o: &Self) -> bool {
    self.0.id() == o.0.id()
  }
}
impl Eq for MediaProtocol {}
impl serde::Serialize for MediaProtocol {
  fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(self.0.id())
  }
}
impl<'de> serde::Deserialize<'de> for MediaProtocol {
  fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
    let id = <String as serde::Deserialize>::deserialize(d)?;
    crate::extensions::media_registry::drivers()
      .iter()
      .find(|driver| driver.id() == id)
      .map(|driver| Self(*driver))
      .ok_or_else(|| serde::de::Error::custom(format!("unknown media implementation: {id}")))
  }
}
