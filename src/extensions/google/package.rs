//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "google",
  tools: &[
    &super::search_tool::REGISTRATION,
    &super::image_tool::REGISTRATION,
    &super::interactions_image_tool::REGISTRATION,
  ],
  images: &[
    &super::antigravity_image_driver::DRIVER,
    &super::image_driver::DRIVER,
    &super::gemini_interactions_driver::DRIVER,
  ],
  media: &[],
  search: &[&super::search::CODEC],
  fetch: &[],
  presets: include_str!("presets.json"),
};
