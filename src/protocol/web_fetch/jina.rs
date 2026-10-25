//! Jina Reader: a key is optional; cache bypass travels as a header.
use super::{FetchRequest, FetchResult, read_page};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};
pub(super) fn build_draft(request: &FetchRequest) -> Result<Draft, Error> {
  let mut draft = Draft::post_json(&json!({"url":request.url}))?;
  if request.refresh {
    draft.headers.push(("x-no-cache".into(), "true".into()));
  }
  Ok(draft)
}
pub(super) fn parse_body(body: &Value) -> Result<FetchResult, Error> {
  read_page(&body["data"], "content")
}
