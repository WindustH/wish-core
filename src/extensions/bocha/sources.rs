//! Supplier-owned web service endpoints and authentication declarations.
use crate::protocol::endpoint::{AuthScheme, Source};
pub fn search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://api.bocha.cn"),
    path: Some("/v1/web-search"),
    auth: AuthScheme::Bearer(None),
    headers: &[],
  }
}
