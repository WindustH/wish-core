//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "openai",
  tools: &[
    &super::search_tool::REGISTRATION,
    &super::fetch_tool::REGISTRATION,
    &super::image_tool::REGISTRATION,
    &super::chatgpt_image_tool::REGISTRATION,
    &super::speech_tool::REGISTRATION,
    &super::transcribe_tool::REGISTRATION,
  ],
  images: &[&super::image_driver::DRIVER, &super::codex_driver::DRIVER],
  media: &[&super::openai_speech_driver::DRIVER, &super::openai_transcription_driver::DRIVER],
  search: &[&super::search::CODEC],
  fetch: &[&super::fetch::CODEC],
  presets: include_str!("presets.json"),
};
