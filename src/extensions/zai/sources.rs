//! Supplier-owned web service endpoints and authentication declarations.
use crate::protocol::endpoint::{AuthScheme, Source, SourceHeader};
pub fn search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://api.z.ai/api/paas/v4"),
    path: Some("/web_search"),
    auth: AuthScheme::Bearer(None),
    headers: &[],
  }
}
pub fn fetch(id: &'static str, _has_key: bool) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://api.z.ai/api/paas/v4"),
    path: Some("/reader"),
    auth: AuthScheme::Bearer(None),
    headers: &[SourceHeader::Literal("accept", "application/json")],
  }
}
