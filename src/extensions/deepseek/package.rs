//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "deepseek",
  tools: &[
    &super::search_tool::REGISTRATION,
    &super::fetch_tool::REGISTRATION,
    &super::speech_tool::REGISTRATION,
    &super::transcribe_tool::REGISTRATION,
  ],
  images: &[],
  media: &[&super::speech_driver::DRIVER, &super::transcription_driver::DRIVER],
  search: &[&super::search::CODEC],
  fetch: &[&super::fetch::CODEC],
  presets: include_str!("presets.json"),
};
