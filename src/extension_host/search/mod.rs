//! Registry-backed read execution. Supplier logic is defined by its extension.
mod client;
mod source;
use crate::extensions::shared::read_codecs::SearchCodec;
pub use crate::extensions::shared::web_search::{
  Recency, SearchQuery, SearchResult, SearchResults,
};
use crate::protocol::{
  endpoint::{Credentials, Draft},
  error::Error,
};
use serde_json::Value;
pub mod fetch;
pub use fetch::search;
#[derive(Clone, Copy)]
pub struct SearchProtocol(pub &'static SearchCodec);
impl SearchProtocol {
  pub fn get_id(self) -> &'static str {
    self.0.id
  }

  pub fn get_default_base_url(self) -> Option<&'static str> {
    (self.0.source)(self.get_id()).base_url
  }
  pub fn can_filter_recency(self) -> bool {
    self.0.recency
  }
  fn build_draft(
    self,
    query: &SearchQuery,
    now: u64,
    credentials: &Credentials,
  ) -> Result<Draft, Error> {
    (self.0.build)(query, now, credentials)
  }
  fn explain_refusal(self, status: u16) -> Option<Error> {
    (self.0.refusal)(status)
  }
  fn parse_body(self, body: &Value) -> Result<Vec<SearchResult>, Error> {
    (self.0.parse)(body)
  }
}
impl std::fmt::Debug for SearchProtocol {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.write_str(self.0.id)
  }
}
impl PartialEq for SearchProtocol {
  fn eq(&self, other: &Self) -> bool {
    self.0.id == other.0.id
  }
}
impl Eq for SearchProtocol {}
impl std::hash::Hash for SearchProtocol {
  fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
    self.0.id.hash(state)
  }
}
impl std::str::FromStr for SearchProtocol {
  type Err = Error;
  fn from_str(id: &str) -> Result<Self, Error> {
    crate::extensions::read_registry::search()
      .iter()
      .find(|codec| codec.id == id)
      .map(|codec| Self(codec))
      .ok_or_else(|| Error::Build(format!("unknown web search protocol: {id}")))
  }
}
impl serde::Serialize for SearchProtocol {
  fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(self.0.id)
  }
}
impl<'de> serde::Deserialize<'de> for SearchProtocol {
  fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
    let id = <String as serde::Deserialize>::deserialize(d)?;
    id.parse().map_err(serde::de::Error::custom)
  }
}
