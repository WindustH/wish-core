//! DeepSeek webpage fetch request and response.
use crate::extensions::shared::web_fetch::{FetchResult, read_page};
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::{Value, json};
pub(crate) fn build_draft(request: &Request) -> Result<Draft, Error> {
  Draft::post_json(&json!({"url":request.url}))
}
pub(crate) fn parse_body(body: &Value) -> Result<FetchResult, Error> {
  read_page(body, "content")
}

#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Request {
  pub url: String,
}
