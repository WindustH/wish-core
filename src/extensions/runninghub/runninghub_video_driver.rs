//! Supplier-owned media execution contract.
use crate::extensions::media_runtime::*;
use crate::extensions::shared::parameters::decode;
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub struct RunninghubVideo;
pub static DRIVER: RunninghubVideo = RunninghubVideo;
impl MediaDriver for RunninghubVideo {
  fn id(&self) -> &'static str {
    "runninghub_video"
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
    super::video::submission(model, prompt, VideoMode::Text, &decode(p)?, &[])
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
    if modify.is_some() {
      return Err(Error::Build("modify_mode is unsupported by this extension".into()));
    }
    super::video::submission(model, prompt, mode, &decode(p)?, inputs)
  }
  fn requires_upload(&self, bytes: usize) -> bool {
    let _ = bytes;
    true
  }
  fn upload<'a>(
    &'a self,
    bytes: Vec<u8>,
    mime: &'a str,
    base: &'a str,
    c: &'a Credentials,
    transport: &'a crate::transport::ReqwestTransport,
    _status_transport: &'a crate::transport::ReqwestTransport,
  ) -> Upload<'a> {
    Box::pin(async move { super::upload::send(bytes, mime, base, c, transport).await.map(Some) })
  }
}
