//! Runner lookup derived from extension packages.
use super::image_runtime::ImageDriver;
pub fn drivers() -> &'static [&'static dyn ImageDriver] {
  static ITEMS: std::sync::OnceLock<Vec<&'static dyn ImageDriver>> = std::sync::OnceLock::new();
  ITEMS.get_or_init(|| {
    super::package::packages().iter().flat_map(|p| p.images.iter().copied()).collect()
  })
}
