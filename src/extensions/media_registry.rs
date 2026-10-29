//! Runner lookup derived from extension packages.
use super::media_runtime::MediaDriver;
pub fn drivers() -> &'static [&'static dyn MediaDriver] {
  static ITEMS: std::sync::OnceLock<Vec<&'static dyn MediaDriver>> = std::sync::OnceLock::new();
  ITEMS.get_or_init(|| {
    super::package::packages().iter().flat_map(|p| p.media.iter().copied()).collect()
  })
}
