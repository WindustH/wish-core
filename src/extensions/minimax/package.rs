//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "minimax",
  tools: &[&super::search_tool::REGISTRATION, &super::video_tool::REGISTRATION, &super::SPEECH],
  images: &[],
  media: &[&super::minimax_h3_driver::DRIVER, &super::minimax_speech_driver::DRIVER],
  search: &[&super::search::CODEC],
  fetch: &[],
  presets: include_str!("presets.json"),
};
