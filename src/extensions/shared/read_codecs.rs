//! Optional read execution primitives. Tool contracts remain owned by each extension.
use super::{
  web_fetch::{FetchRequest, FetchResult},
  web_search::{SearchQuery, SearchResult},
};
use crate::protocol::{
  endpoint::{Draft, Source},
  error::Error,
};
use serde_json::Value;
pub struct SearchCodec {
  pub id: &'static str,
  pub source: fn(&'static str) -> Source,
  pub recency: bool,
  pub build: fn(&SearchQuery, u64) -> Result<Draft, Error>,
  pub parse: fn(&Value) -> Result<Vec<SearchResult>, Error>,
  pub refusal: fn(u16) -> Option<Error>,
}
pub struct FetchCodec {
  pub id: &'static str,
  pub source: fn(&'static str, bool) -> Source,
  pub build: fn(&FetchRequest) -> Result<Draft, Error>,
  pub parse: fn(&Value) -> Result<FetchResult, Error>,
}
