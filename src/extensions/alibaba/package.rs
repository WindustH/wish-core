//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "alibaba",
  tools: &[&super::wan_image_tool::REGISTRATION],
  images: &[&super::image_driver::DRIVER],
  media: &[],
  search: &[],
  fetch: &[],
  presets: include_str!("presets.json"),
};
