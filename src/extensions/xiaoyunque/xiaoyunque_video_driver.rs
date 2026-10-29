//! Supplier-owned media execution contract.
use crate::extensions::media_runtime::*;
use crate::extensions::shared::parameters::decode;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct XiaoyunqueVideo;
pub static DRIVER: XiaoyunqueVideo = XiaoyunqueVideo;
impl MediaDriver for XiaoyunqueVideo {
  fn id(&self) -> &'static str {
    "xiaoyunque_video"
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
    super::video::submission(model, prompt, &decode(p)?, &[])
  }
  fn task_id(&self, v: &Value) -> Result<String, String> {
    super::video::task_id(v)
  }
  fn status_request(&self, task: &str) -> Result<(String, Draft), Error> {
    super::video::status_request(task)
  }
  fn decode_status(&self, v: &Value) -> Result<VideoStatus, Error> {
    super::video::status(v)
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
  fn video_submission(
    &self,
    model: &str,
    prompt: &str,
    _mode: VideoMode,
    p: &Parameters,
    inputs: &[ResolvedInput],
    modify: Option<&str>,
  ) -> Result<(String, Draft), Error> {
    if modify.is_some() {
      return Err(Error::Build("modify_mode is unsupported by this extension".into()));
    }
    super::video::submission(model, prompt, &decode(p)?, inputs)
  }
}
