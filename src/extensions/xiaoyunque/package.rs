//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "xiaoyunque",
  tools: &[&super::video_tool::REGISTRATION],
  images: &[],
  media: &[&super::xiaoyunque_video_driver::DRIVER],
  search: &[],
  fetch: &[],
  presets: include_str!("presets.json"),
};
