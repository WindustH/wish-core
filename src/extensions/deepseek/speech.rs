//! Whole-text TTS. Request WAV so the streamed response is a playable session artifact.
use crate::protocol::{endpoint::Draft, error::Error};
use serde_json::json;

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
  pub voice: Option<String>,
  pub filter_markdown: Option<bool>,
}

pub fn submission(text: &str, options: &Options) -> Result<(String, Draft), Error> {
  // DeepSeek counts each Han character twice; other Unicode characters count once.
  let billable: usize = text
    .chars()
    .take(10_001)
    .map(|c| {
      if matches!(c as u32, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x20000..=0x323af)
      {
        2
      } else {
        1
      }
    })
    .sum();
  if text.trim().is_empty() || billable > 10_000 {
    return Err(Error::Build(
      "DeepSeek speech requires 1..10000 billing characters (Han characters count twice)".into(),
    ));
  }
  let voice = options.voice.as_deref().unwrap_or("stella");
  if !matches!(voice, "stella" | "tide") {
    return Err(Error::Build("DeepSeek speech voice must be stella or tide".into()));
  }
  let draft = Draft::post_json(&json!({
    "text": text,
    "voice_id": voice,
    "format": "wav",
    "filter_markdown": options.filter_markdown.unwrap_or(true),
  }))?;
  if draft.body.len() > 64 * 1024 {
    return Err(Error::Build("DeepSeek speech request exceeds 64 KiB".into()));
  }
  Ok(("/v1/tools/tts".into(), draft))
}
