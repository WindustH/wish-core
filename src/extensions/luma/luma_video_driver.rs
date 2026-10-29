//! Supplier-owned media execution contract.
use crate::extensions::media_runtime::*;
use crate::extensions::shared::parameters::decode;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct LumaVideo;
pub static DRIVER: LumaVideo = LumaVideo;
impl MediaDriver for LumaVideo {
  fn id(&self) -> &'static str {
    "luma_video"
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
    let (path, body) = super::video::submission(model, prompt, &decode(p)?)?;
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
    super::video_contract::capabilities(model)
  }
  fn validate_video_inputs(
    &self,
    model: &str,
    mode: VideoMode,
    inputs: &[VideoInput],
  ) -> Result<(), Error> {
    super::video_contract::validate(model, mode, inputs)
  }
  fn parameters(
    &self,
    defaults: &Parameters,
    overrides: &Parameters,
    mode: VideoMode,
    _inputs: &[VideoInput],
  ) -> Result<Parameters, Error> {
    super::video_contract::parameters(defaults, overrides, mode)
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
    super::video::with_inputs(model, prompt, mode, &decode(p)?, inputs, modify)
  }
}
