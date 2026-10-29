//! Webpage reading across services. Protocol adapters own wire formats; callers own
//! credentials, transport policy, retries, provider selection and saved snapshots.
mod client;
use crate::extensions::deepseek::fetch as deepseek;
use crate::extensions::openai::fetch as codex;
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::Value;
pub mod execution;
use crate::extensions::jina::fetch as jina;
mod source;
use crate::extensions::zai::fetch as zai;
pub use execution::fetch;

#[derive(Clone, Debug, Default)]
pub struct FetchRequest {
  pub url: String,
  pub refresh: bool,
  pub conversation: Option<String>,
  pub model: Option<String>,
}
pub use crate::extensions::shared::web_fetch::FetchResult;
crate::protocol::text_id_enum! {
  /// Supported webpage-reading wire protocols, independent of configured providers.
  #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
  pub enum FetchProtocol (unknown: "unknown web fetch protocol: {}") {
    DeepseekFetch => "deepseek_fetch",
    ZaiReader => "zai_reader",
    JinaReader => "jina_reader",
    CodexFetch => "codex_fetch",
  }
}
impl FetchProtocol {
  pub fn build_draft(self, request: &FetchRequest) -> Result<Draft, Error> {
    match self {
      Self::DeepseekFetch => deepseek::build_draft(&request.into()),
      Self::ZaiReader => zai::build_draft(&request.into()),
      Self::JinaReader => jina::build_draft(&request.into()),
      Self::CodexFetch => codex::build_draft(&request.into()),
    }
  }
  pub fn parse_body(self, body: &Value) -> Result<FetchResult, Error> {
    match self {
      Self::DeepseekFetch => deepseek::parse_body(body),
      Self::ZaiReader => zai::parse_body(body),
      Self::JinaReader => jina::parse_body(body),
      Self::CodexFetch => codex::parse_body(body),
    }
  }
}

impl From<&FetchRequest> for crate::extensions::jina::fetch::Request {
  fn from(value: &FetchRequest) -> Self {
    Self { url: value.url.clone(), refresh: value.refresh }
  }
}
impl From<&FetchRequest> for crate::extensions::zai::fetch::Request {
  fn from(value: &FetchRequest) -> Self {
    Self { url: value.url.clone(), refresh: value.refresh }
  }
}
impl From<&FetchRequest> for crate::extensions::openai::fetch::Request {
  fn from(value: &FetchRequest) -> Self {
    Self {
      url: value.url.clone(),
      conversation: value.conversation.clone(),
      model: value.model.clone(),
    }
  }
}
impl From<&FetchRequest> for crate::extensions::deepseek::fetch::Request {
  fn from(value: &FetchRequest) -> Self {
    Self { url: value.url.clone() }
  }
}
