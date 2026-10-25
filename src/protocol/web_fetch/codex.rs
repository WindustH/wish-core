//! Codex SearchClient page open. Verified against openai/codex 970111f6,
//! codex-api/src/search.rs and endpoint/search.rs, and an authenticated read.
use super::{FetchRequest, FetchResult, read_page};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};
pub(super) fn build_draft(request: &FetchRequest) -> Result<Draft, Error> {
  Draft::post_json(
    &json!({"id":request.conversation.as_deref().unwrap_or(""),"model":request.model.as_deref().unwrap_or("gpt-5.5"),"commands":{"open":[{"ref_id":request.url}]},"response_length":"long"}),
  )
}
pub(super) fn parse_body(body: &Value) -> Result<FetchResult, Error> {
  read_page(body, "output")
}
