//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "together",
  tools: &[&super::image_tool::REGISTRATION],
  images: &[&super::image_driver::DRIVER],
  media: &[],
  search: &[],
  fetch: &[],
  presets: include_str!("presets.json"),
};
