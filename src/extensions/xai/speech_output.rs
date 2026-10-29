//! xAI audio output and timed response validation.
use crate::protocol::error::Error;
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpeechOutput {
  pub codec: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub sample_rate: Option<u32>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub bit_rate: Option<u32>,
}
impl SpeechOutput {
  pub(crate) fn validate(&self) -> Result<(), Error> {
    if !["mp3", "wav"].contains(&self.codec.as_str()) {
      return Err(Error::Build("speech_output.codec must be mp3 or wav for stored audio".into()));
    }
    if self.sample_rate.is_some_and(|v| ![8000, 16000, 22050, 24000, 44100, 48000].contains(&v)) {
      return Err(Error::Build("unsupported speech sample rate".into()));
    }
    if self
      .bit_rate
      .is_some_and(|v| self.codec != "mp3" || ![32000, 64000, 96000, 128000, 192000].contains(&v))
    {
      return Err(Error::Build("unsupported bit rate (MP3 only)".into()));
    }
    Ok(())
  }
}

/// Decode the timed JSON envelope off the async runtime. Do not persist a second copy of base64 audio.
pub fn decode_timed(bytes: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
  use base64::Engine;
  #[derive(Serialize, Deserialize)]
  struct Timings {
    graph_chars: Vec<String>,
    graph_times: Vec<[f64; 2]>,
  }
  #[derive(Deserialize)]
  struct Envelope<'a> {
    #[serde(borrow)]
    audio: &'a str,
    content_type: String,
    duration: f64,
    audio_timestamps: Timings,
  }
  let envelope: Envelope<'_> =
    serde_json::from_slice(bytes).map_err(|_| "invalid timed speech envelope")?;
  let times = &envelope.audio_timestamps;
  if !envelope.duration.is_finite()
    || envelope.duration < 0.0
    || times.graph_chars.len() != times.graph_times.len()
    || times.graph_chars.len() > 240_000
    || times.graph_times.iter().any(|[start, end]| {
      !start.is_finite()
        || !end.is_finite()
        || *start < 0.0
        || end < start
        || *end > envelope.duration + 0.001
    })
  {
    return Err("invalid speech timing data".into());
  }
  if envelope.audio.len() > (128 * 1024 * 1024u64).div_ceil(3) as usize * 4 {
    return Err("timed speech audio exceeds 128 MiB".into());
  }
  let audio = base64::engine::general_purpose::STANDARD
    .decode(envelope.audio)
    .map_err(|_| "invalid speech audio encoding")?;
  if audio.len() > 128 * 1024 * 1024 {
    return Err("timed speech audio exceeds 128 MiB".into());
  }
  let metadata = serde_json::to_vec(&serde_json::json!({"content_type":envelope.content_type,"duration":envelope.duration,"audio_timestamps":envelope.audio_timestamps})).map_err(|e|e.to_string())?;
  Ok((audio, metadata))
}
