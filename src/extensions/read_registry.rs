//! Reader lookup derived from extension packages.
use super::shared::read_codecs::{FetchCodec, SearchCodec};
pub fn search() -> &'static [&'static SearchCodec] {
  static ITEMS: std::sync::OnceLock<Vec<&'static SearchCodec>> = std::sync::OnceLock::new();
  ITEMS.get_or_init(|| {
    super::package::packages().iter().flat_map(|p| p.search.iter().copied()).collect()
  })
}
pub fn fetch() -> &'static [&'static FetchCodec] {
  static ITEMS: std::sync::OnceLock<Vec<&'static FetchCodec>> = std::sync::OnceLock::new();
  ITEMS.get_or_init(|| {
    super::package::packages().iter().flat_map(|p| p.fetch.iter().copied()).collect()
  })
}
