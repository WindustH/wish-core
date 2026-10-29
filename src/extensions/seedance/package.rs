//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "seedance",
  tools: &[&super::seedream_image_tool::REGISTRATION, &super::video_tool::REGISTRATION],
  images: &[&super::image_driver::DRIVER],
  media: &[&super::seedance_video_driver::DRIVER, &super::seedance25_video_driver::DRIVER],
  search: &[],
  fetch: &[],
  presets: include_str!("presets.json"),
};
