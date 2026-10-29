//! Supplier-owned web service endpoints and authentication declarations.
use crate::protocol::endpoint::{AuthScheme, Source, SourceHeader};
pub fn search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://api.tavily.com"),
    path: Some("/search"),
    auth: AuthScheme::Bearer(None),
    headers: &[],
  }
}
pub fn keyless_search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://api.tavily.com"),
    path: Some("/search"),
    auth: AuthScheme::None,
    headers: &[SourceHeader::Literal("x-tavily-access-mode", "keyless")],
  }
}
