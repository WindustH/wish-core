//! Page presentation record and JSON reader.
use crate::protocol::error::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchResult {
  pub title: String,
  pub content: String,
}
pub(crate) fn read_page(body: &Value, field: &str) -> Result<FetchResult, Error> {
  let content = body[field]
    .as_str()
    .ok_or_else(|| Error::Malformed("fetch provider returned no page content".into()))?;
  Ok(FetchResult { title: body["title"].as_str().unwrap_or("").into(), content: content.into() })
}

#[derive(Clone, Debug, Default)]
pub struct FetchRequest {
  pub url: String,
  pub refresh: bool,
  pub conversation: Option<String>,
  pub model: Option<String>,
}
