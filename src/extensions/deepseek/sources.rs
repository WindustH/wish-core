//! Supplier-owned web service endpoints and authentication declarations.
use crate::protocol::endpoint::{AuthScheme, Source, SourceHeader};
pub fn search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://api.deepseek.com"),
    path: Some("/v1/tools/search"),
    auth: AuthScheme::Bearer(None),
    headers: &[],
  }
}
pub fn fetch(id: &'static str, _has_key: bool) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://api.deepseek.com"),
    path: Some("/v1/tools/fetch"),
    auth: AuthScheme::Bearer(None),
    headers: &[SourceHeader::Literal("accept", "application/json")],
  }
}
