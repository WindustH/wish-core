//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "runway",
  tools: &[&super::REGISTRATION],
  images: &[],
  media: &[&super::runway_video_driver::DRIVER],
  search: &[],
  fetch: &[],
  presets: include_str!("presets.json"),
};
