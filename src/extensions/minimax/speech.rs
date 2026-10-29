//! Native MiniMax request construction; single and mixed voices share this implementation.
use super::{Voice, parameters::Parameters};
use crate::protocol::endpoint::Draft;
use serde_json::json;

impl Voice {
  pub fn validate_controls(&self, model: &str) -> Result<(), String> {
    if self.speed.is_some_and(|v| !v.is_finite() || !(0.5..=2.0).contains(&v)) {
      return Err("MiniMax speech speed must be 0.5..2".into());
    }
    if self.volume.is_some_and(|v| !v.is_finite() || v <= 0.0 || v > 10.0) {
      return Err("MiniMax speech volume must be greater than 0 and at most 10".into());
    }
    if self.pitch.is_some_and(|v| !(-12..=12).contains(&v)) {
      return Err("MiniMax speech pitch must be an integer from -12 to 12".into());
    }
    if let Some(emotion) = self.emotion.as_deref() {
      let supported = ["happy", "sad", "angry", "fearful", "disgusted", "surprised", "calm"]
        .contains(&emotion)
        || (["fluent", "whisper"].contains(&emotion)
          && ["speech-2.6-hd", "speech-2.6-turbo"].contains(&model));
      if !supported {
        return Err("emotion is not supported by this MiniMax speech model".into());
      }
    }
    Ok(())
  }
}

pub fn submission(
  model: &str,
  text: &str,
  voice: &Voice,
  parameters: &Parameters,
) -> Result<(String, Draft), String> {
  if text.trim().is_empty() || text.chars().count() > 9999 {
    return Err("MiniMax speech text must contain 1..9999 characters".into());
  }
  voice.validate_controls(model)?;
  let id = if let Some(weights) = &parameters.timbre_weights {
    if voice.id.as_deref().is_some_and(|id| !id.is_empty()) {
      return Err("timbre_weights cannot be combined with a single voice.id".into());
    }
    if weights.is_empty() || weights.len() > 4 {
      return Err("MiniMax timbre_weights requires 1..4 voices".into());
    }
    let mut ids = std::collections::HashSet::new();
    for entry in weights {
      if entry.voice_id.trim().is_empty()
        || entry.voice_id.len() > 200
        || !(1..=100).contains(&entry.weight)
        || !ids.insert(&entry.voice_id)
      {
        return Err(
          "mixed voices require distinct nonempty voice IDs and integer weights from 1 to 100"
            .into(),
        );
      }
    }
    ""
  } else {
    voice
      .id
      .as_deref()
      .filter(|v| !v.trim().is_empty() && v.len() <= 200)
      .ok_or("speech requires a configured or requested voice ID")?
  };
  let mut setting = json!({"voice_id":id});
  for (key, value) in [
    ("speed", json!(voice.speed)),
    ("vol", json!(voice.volume)),
    ("pitch", json!(voice.pitch)),
    ("emotion", json!(voice.emotion)),
  ] {
    if !value.is_null() {
      setting[key] = value;
    }
  }
  let mut body = json!({"model":model,"text":text,"stream":false,"output_format":"url",
    "voice_setting":setting,"language_boost":"auto",
    "audio_setting":{"format":"mp3","sample_rate":32000,"bitrate":128000,"channel":1}});
  parameters.apply(model, &mut body)?;
  Ok(("/v1/t2a_v2".into(), Draft::post_json(&body).map_err(|e| e.to_string())?))
}

/// Only the extension interprets the upstream completion envelope.
pub struct ResultUrls {
  pub audio: String,
  pub subtitles: Option<String>,
}
pub fn result_urls(value: &serde_json::Value, subtitles: bool) -> Result<ResultUrls, String> {
  if value["base_resp"]["status_code"].as_u64() != Some(0)
    || value["data"]["status"].as_u64() != Some(2)
  {
    return Err("MiniMax speech did not return a completed result".into());
  }
  let audio =
    value["data"]["audio"].as_str().ok_or("MiniMax speech result is missing its audio URL")?;
  crate::extensions::shared::urls::validate_url(audio).map_err(|e| e.to_string())?;
  Ok(ResultUrls {
    audio: audio.to_owned(),
    subtitles: subtitles.then(|| subtitle_url(value)).transpose()?,
  })
}
/// Keep upstream subtitle URLs private in the journal, and expose only owned JSON blobs.
fn subtitle_url(value: &serde_json::Value) -> Result<String, String> {
  let url =
    value["data"]["subtitle_file"].as_str().ok_or("MiniMax did not return requested subtitles")?;
  crate::extensions::shared::urls::validate_url(url).map_err(|e| e.to_string())?;
  Ok(url.to_owned())
}
pub fn validate_subtitles(bytes: &[u8]) -> Result<(), String> {
  let value: serde_json::Value =
    serde_json::from_slice(bytes).map_err(|_| "invalid MiniMax subtitle JSON")?;
  if !value.is_array() && !value.is_object() {
    return Err("MiniMax subtitles must be a JSON array or object".into());
  }
  Ok(())
}

/// The MiniMax service's own persisted voice defaults; unrelated suppliers cannot accept these fields.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct VoiceOptions {
  #[serde(default)]
  voice: Option<String>,
  #[serde(default)]
  speed: Option<f64>,
  #[serde(default)]
  volume: Option<f64>,
  #[serde(default)]
  pitch: Option<i32>,
  #[serde(default)]
  emotion: Option<String>,
}
pub(crate) fn configured_submission(
  model: &str,
  text: &str,
  options: &crate::extensions::shared::parameters::Parameters,
  parameters: &Parameters,
) -> Result<(String, Draft), crate::protocol::error::Error> {
  let o: VoiceOptions = crate::extensions::shared::parameters::decode(options)?;
  submission(
    model,
    text,
    &Voice { id: o.voice, speed: o.speed, volume: o.volume, pitch: o.pitch, emotion: o.emotion },
    parameters,
  )
  .map_err(crate::protocol::error::Error::Build)
}

/// Native speech plan keeps text, voice selection and advanced controls together.
pub struct Plan {
  pub text: String,
  pub voice: Voice,
  pub parameters: Box<Parameters>,
}
impl Plan {
  pub fn build(
    &self,
    model: &str,
    defaults: &crate::extensions::shared::parameters::Parameters,
  ) -> Result<(String, Draft), crate::protocol::error::Error> {
    let defaults: VoiceOptions = crate::extensions::shared::parameters::decode(defaults)?;
    let voice = Voice {
      id: self
        .voice
        .id
        .clone()
        .or_else(|| if self.parameters.timbre_weights.is_some() { None } else { defaults.voice }),
      speed: self.voice.speed.or(defaults.speed),
      volume: self.voice.volume.or(defaults.volume),
      pitch: self.voice.pitch.or(defaults.pitch),
      emotion: self.voice.emotion.clone().or(defaults.emotion),
    };
    submission(model, &self.text, &voice, &self.parameters)
      .map_err(crate::protocol::error::Error::Build)
  }
  pub fn result_urls(&self, value: &serde_json::Value) -> Result<ResultUrls, String> {
    result_urls(value, self.parameters.subtitle_enable == Some(true))
  }
}

pub fn validate_configuration(
  model: &str,
  values: &crate::extensions::shared::parameters::Parameters,
) -> Result<(), crate::protocol::error::Error> {
  let mut values = values.clone();
  if values.get("voice").is_none_or(serde_json::Value::is_null) {
    values.insert("voice".into(), serde_json::json!("validation"));
  }
  configured_submission(model, "validation", &values, &Parameters::default()).map(|_| ())
}

impl crate::extensions::media_runtime::NativePlan for Plan {
  fn driver_id(&self) -> &'static str {
    "minimax_speech"
  }
  fn build(
    &self,
    c: &crate::extensions::media_runtime::PlanContext<'_>,
  ) -> Result<(String, Draft), crate::protocol::error::Error> {
    self.build(c.model, c.defaults)
  }
  fn buffered_audio(&self) -> bool {
    true
  }
  fn decode_audio(
    &self,
    bytes: &[u8],
  ) -> Result<crate::extensions::media_runtime::AudioCompletion, String> {
    let value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let urls = self.result_urls(&value)?;
    Ok(crate::extensions::media_runtime::AudioCompletion::Download {
      audio: urls.audio,
      subtitles: urls.subtitles,
    })
  }
}
