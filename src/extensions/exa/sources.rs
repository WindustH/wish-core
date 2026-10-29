//! Supplier-owned web service endpoints and authentication declarations.
use crate::protocol::endpoint::{AuthScheme, Source};
pub fn search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://api.exa.ai"),
    path: Some("/search"),
    auth: AuthScheme::Header("x-api-key"),
    headers: &[],
  }
}
