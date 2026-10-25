//! Extracts the session bridge credential from an HTTP request.
use axum::http::{HeaderMap, header::AUTHORIZATION};
pub fn bearer_token(headers: &HeaderMap) -> Option<&str> {
  headers
    .get(AUTHORIZATION)
    .and_then(|value| value.to_str().ok())
    .and_then(|value| value.strip_prefix("Bearer "))
}
