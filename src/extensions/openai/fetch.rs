//! Codex SearchClient page open. Verified against openai/codex 970111f6,
//! codex-api/src/search.rs and endpoint/search.rs, and an authenticated read.
use crate::extensions::shared::web_fetch::{FetchResult, read_page};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};
pub(crate) fn build_draft(request: &Request) -> Result<Draft, Error> {
  Draft::post_json(
    &json!({"id":request.conversation.as_deref().unwrap_or(""),"model":request.model.as_deref().unwrap_or("gpt-5.5"),"commands":{"open":[{"ref_id":request.url}],"response_length":"long"}}),
  )
}
pub(crate) fn parse_body(body: &Value) -> Result<FetchResult, Error> {
  read_page(body, "output")
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
  pub url: String,
  pub conversation: Option<String>,
  pub model: Option<String>,
}

use crate::extensions::shared::web_fetch::FetchRequest;
impl From<&FetchRequest> for Request {
  fn from(value: &FetchRequest) -> Self {
    Self {
      url: value.url.clone(),
      conversation: value.conversation.clone(),
      model: value.model.clone(),
    }
  }
}

pub static CODEC: crate::extensions::shared::read_codecs::FetchCodec =
  crate::extensions::shared::read_codecs::FetchCodec {
    id: "codex_fetch",
    source: super::sources::fetch,
    build: |request| build_draft(&request.into()),
    parse: parse_body,
  };
