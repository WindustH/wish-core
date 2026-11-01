use axum::http::{HeaderMap, header};
pub(super) fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
  headers
    .get_all(header::IF_NONE_MATCH)
    .iter()
    .filter_map(|value| value.to_str().ok())
    .flat_map(|values| values.split(','))
    .any(|value| {
      let value = value.trim();
      value == "*" || value.strip_prefix("W/").unwrap_or(value) == etag
    })
}
