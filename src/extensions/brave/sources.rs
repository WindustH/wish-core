//! Supplier-owned web service endpoints and authentication declarations.
use crate::protocol::endpoint::{AuthScheme, Source, SourceHeader};
pub fn search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://api.search.brave.com"),
    path: Some("/res/v1/web/search"),
    auth: AuthScheme::Header("x-subscription-token"),
    headers: &[SourceHeader::Literal("accept", "application/json")],
  }
}
