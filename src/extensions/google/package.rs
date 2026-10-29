//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "google",
  tools: &[&super::image_tool::REGISTRATION, &super::interactions_image_tool::REGISTRATION],
  images: &[&super::image_driver::DRIVER, &super::gemini_interactions_driver::DRIVER],
  media: &[],
  search: &[],
  fetch: &[],
  presets: include_str!("presets.json"),
};
