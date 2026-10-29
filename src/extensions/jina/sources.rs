//! Supplier-owned web service endpoints and authentication declarations.
use crate::protocol::endpoint::{AuthScheme, Source, SourceHeader};
pub fn search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://s.jina.ai"),
    path: Some("/"),
    auth: AuthScheme::Bearer(None),
    headers: &[],
  }
}
pub fn fetch(id: &'static str, has_key: bool) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://r.jina.ai"),
    path: Some("/"),
    auth: if has_key { AuthScheme::Bearer(None) } else { AuthScheme::None },
    headers: &[SourceHeader::Literal("accept", "application/json")],
  }
}
