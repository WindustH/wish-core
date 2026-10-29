//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "siliconflow",
  tools: &[&super::image_tool::REGISTRATION, &super::transcribe_tool::REGISTRATION],
  images: &[&super::image_driver::DRIVER],
  media: &[&super::silicon_transcription_driver::DRIVER],
  search: &[],
  fetch: &[],
  presets: include_str!("presets.json"),
};
