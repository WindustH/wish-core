//! Webpage reading across services. Protocol adapters own wire formats; callers own
//! credentials, transport policy, retries, provider selection and saved snapshots.
use crate::protocol::{endpoint::Draft, error::Error};
use serde::{Deserialize, Serialize};
use serde_json::Value;
mod codex;
mod deepseek;
pub mod fetch;
mod jina;
mod source;
mod zai;
pub use fetch::fetch;

#[derive(Clone, Debug, Default)]
pub struct FetchRequest {
  pub url: String,
  pub refresh: bool,
  pub conversation: Option<String>,
  pub model: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchResult {
  pub title: String,
  pub content: String,
}
text_id_enum! {
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
      Self::DeepseekFetch => deepseek::build_draft(request),
      Self::ZaiReader => zai::build_draft(request),
      Self::JinaReader => jina::build_draft(request),
      Self::CodexFetch => codex::build_draft(request),
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
fn read_page(body: &Value, field: &str) -> Result<FetchResult, Error> {
  let content = body[field]
    .as_str()
    .ok_or_else(|| Error::Malformed("fetch provider returned no page content".into()))?;
  Ok(FetchResult { title: body["title"].as_str().unwrap_or("").into(), content: content.into() })
}

#[cfg(test)]
mod tests;
