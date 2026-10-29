//! Complete bundled extension registration: tools, optional runners and configuration defaults.
use crate::extensions::package::Package;
pub const PACKAGE: Package = Package {
  id: "elevenlabs",
  tools: &[
    &super::speech_tool::REGISTRATION,
    &super::music_tool::REGISTRATION,
    &super::sound_tool::REGISTRATION,
    &super::transcribe_tool::REGISTRATION,
  ],
  images: &[],
  media: &[
    &super::eleven_speech_driver::DRIVER,
    &super::eleven_music_driver::DRIVER,
    &super::eleven_sound_driver::DRIVER,
    &super::eleven_transcription_driver::DRIVER,
  ],
  search: &[],
  fetch: &[],
  presets: include_str!("presets.json"),
};
