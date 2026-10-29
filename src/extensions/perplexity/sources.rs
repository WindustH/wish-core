//! Supplier-owned web service endpoints and authentication declarations.
use crate::protocol::endpoint::{AuthScheme, Source};
pub fn search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: Some("https://api.perplexity.ai"),
    path: Some("/search"),
    auth: AuthScheme::Bearer(None),
    headers: &[],
  }
}
