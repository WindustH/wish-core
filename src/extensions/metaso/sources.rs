//! Supplier-owned web service endpoints and authentication declarations.
use crate::protocol::endpoint::{AuthScheme, Source, SourceHeader};
pub fn search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://metaso.cn"),
    path: Some("/api/v1/search"),
    auth: AuthScheme::Bearer(None),
    headers: &[SourceHeader::Literal("accept", "application/json")],
  }
}
