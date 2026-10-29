//! Supplier-owned media execution contract.
use crate::extensions::media_runtime::*;
use crate::extensions::shared::parameters::decode;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct SeedanceVideo;
pub static DRIVER: SeedanceVideo = SeedanceVideo;
impl MediaDriver for SeedanceVideo {
  fn id(&self) -> &'static str {
    "seedance_video"
  }
  fn kind(&self) -> MediaKind {
    MediaKind::Video
  }
  fn credentials_ready(&self, c: &Credentials) -> bool {
    super::endpoint::ready(c)
  }
  fn call(&self, base: &str, path: &str, c: &Credentials, d: Draft) -> Result<Call, Error> {
    super::endpoint::call(base, path, c, d)
  }
  fn submission(
    &self,
    model: &str,
    prompt: &str,
    p: &Parameters,
  ) -> Result<(String, Draft), Error> {
    let (path, body) = super::video::submission(model, prompt, &decode(p)?, false)?;
    Ok((path, Draft::post_json(&body)?))
  }
  fn task_id(&self, v: &Value) -> Result<String, String> {
    super::video_status::task_id(v)
  }
  fn status_request(&self, task: &str) -> Result<(String, Draft), Error> {
    super::video_status::request(task)
  }
  fn decode_status(&self, v: &Value) -> Result<VideoStatus, Error> {
    super::video_status::decode(v)
  }
  fn video_capabilities(&self, model: &str) -> Option<VideoCapabilities> {
    super::video_contract::capabilities(model, false)
  }
  fn validate_video_inputs(
    &self,
    model: &str,
    mode: VideoMode,
    inputs: &[VideoInput],
  ) -> Result<(), Error> {
    super::video_contract::validate(model, mode, inputs, false)
  }
  fn parameters(
    &self,
    defaults: &Parameters,
    overrides: &Parameters,
    mode: VideoMode,
    _inputs: &[VideoInput],
  ) -> Result<Parameters, Error> {
    super::video_contract::parameters(defaults, overrides, mode, false)
  }
  fn video_submission(
    &self,
    model: &str,
    prompt: &str,
    mode: VideoMode,
    p: &Parameters,
    inputs: &[ResolvedInput],
    modify: Option<&str>,
  ) -> Result<(String, Draft), Error> {
    if modify.is_some() {
      return Err(Error::Build("modify_mode is unsupported by this extension".into()));
    }
    super::video::with_inputs(model, prompt, mode, &decode(p)?, inputs, false)
  }
  fn validate_input_url(&self, value: &str) -> Result<(), String> {
    super::video_contract::validate_input_url(value)
  }
}
