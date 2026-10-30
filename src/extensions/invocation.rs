//! Erased execution dispatch; each tool retains its own concrete request and implementation.
pub use super::shared::operation::{Failure, Report};
use super::{
  image_runtime::ImageRequest, media_runtime::MediaRequest, shared::web_search::SearchQuery,
  transcription_runtime::Transcription,
};
use std::pin::Pin;
pub type Future<'a> =
  Pin<Box<dyn std::future::Future<Output = Result<Report, Failure>> + Send + 'a>>;
pub struct Read {
  pub url: Option<String>,
  pub cursor: Option<String>,
  pub refresh: bool,
  pub service: Option<String>,
}
/// Scoped host facilities. Permission and service identity are captured by the host, not supplied by a tool.
pub trait Host: Sync {
  fn image(&self, request: ImageRequest) -> Future<'_>;
  fn media(&self, request: MediaRequest) -> Future<'_>;
  fn transcribe(&self, request: Box<dyn Transcription>) -> Future<'_>;
  fn read(&self, request: Read) -> Future<'_>;
  fn search(&self, query: SearchQuery, service: Option<String>) -> Future<'_>;
}
pub trait Executable: Send {
  fn execute<'a>(self: Box<Self>, host: &'a dyn Host) -> Future<'a>;
}
pub struct Invocation(Box<dyn Executable>);
impl Invocation {
  pub fn new(request: impl Executable + 'static) -> Self {
    Self(Box::new(request))
  }
  pub fn execute(self, host: &dyn Host) -> Future<'_> {
    self.0.execute(host)
  }
}
