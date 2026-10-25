//! DeepSeek webpage fetch request and response.
use super::{FetchRequest, FetchResult, read_page};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};
pub(super) fn build_draft(request: &FetchRequest) -> Result<Draft, Error> {
  Draft::post_json(&json!({"url":request.url}))
}
pub(super) fn parse_body(body: &Value) -> Result<FetchResult, Error> {
  read_page(body, "content")
}
