//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "xai",
  tools: &[
    &super::image_tool::REGISTRATION,
    &super::TRANSCRIBE,
    &super::speech::REGISTRATION,
    &super::video::REGISTRATION,
  ],
  images: &[&super::image_driver::DRIVER],
  media: &[
    &super::xai_video_driver::DRIVER,
    &super::xai_speech_driver::DRIVER,
    &super::xai_transcription_driver::DRIVER,
  ],
  search: &[],
  fetch: &[],
  presets: include_str!("presets.json"),
};
