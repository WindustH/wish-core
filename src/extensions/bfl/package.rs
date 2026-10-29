//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "bfl",
  tools: &[&super::GENERATE, &super::FILL],
  images: &[&super::image_driver::DRIVER],
  media: &[],
  search: &[],
  fetch: &[],
  presets: include_str!("presets.json"),
};
