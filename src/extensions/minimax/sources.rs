//! Supplier-owned web service endpoints and authentication declarations.
use crate::protocol::endpoint::{AuthScheme, Source};
pub fn search(id: &'static str) -> Source {
  Source {
    protocol: id,
    base_url: None,
    path: Some("/v1/coding_plan/search"),
    auth: AuthScheme::Bearer(None),
    headers: &[],
  }
}
