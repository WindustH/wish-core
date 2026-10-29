//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "runninghub",
  tools: &[&super::image_tool::REGISTRATION, &super::video_tool::REGISTRATION],
  images: &[&super::image_driver::DRIVER],
  media: &[&super::runninghub_video_driver::DRIVER],
  search: &[],
  fetch: &[],
  presets: include_str!("presets.json"),
};
