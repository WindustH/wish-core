//! Supplier-owned request validation and wire encoding.
use crate::protocol::error::Error;
use serde_json::{Value, json};
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
  pub duration: Option<f64>,
  pub voice: Option<String>,
  pub speed: Option<f64>,
  pub speech_output: Option<crate::extensions::xai::speech_output::SpeechOutput>,
  pub with_timestamps: Option<bool>,
  pub text_normalization: Option<bool>,
  pub pronunciation: Option<std::collections::BTreeMap<String, String>>,
  pub language: Option<String>,
}
pub(crate) fn submission(
  model: &str,
  prompt: &str,
  options: &Options,
) -> Result<(String, Value), Error> {
  options.validate()?;
  let invalid = |s: &str| Error::Build(s.into());
  if prompt.trim().is_empty() || prompt.chars().count() > 60_000 {
    return Err(invalid("prompt is empty or exceeds the service limit"));
  }
  Ok({
    if model != "tts" {
      return Err(invalid("xAI TTS has no selectable model; use tts"));
    }
    if options.duration.is_some() {
      return Err(invalid("speech duration is determined by the text"));
    }
    let voice = options.voice.as_deref().unwrap_or("eve");
    if voice.is_empty()
      || voice.len() > 200
      || !voice.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
      return Err(invalid("invalid xAI voice ID"));
    }
    let language = options.language.as_deref().unwrap_or("auto");
    if language != "auto"
      && !(language.len() <= 16
        && language
          .split('-')
          .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphabetic())))
    {
      return Err(invalid("xAI speech language must be auto or a BCP-47 language code"));
    }
    let mut body = json!({"text":prompt,"voice_id":voice,"language":language});
    if let Some(speed) = options.speed {
      body["speed"] = json!(speed);
    }
    if let Some(output) = &options.speech_output {
      body["output_format"] = json!(output);
    }
    if let Some(enabled) = options.with_timestamps {
      body["with_timestamps"] = json!(enabled);
    }
    if let Some(enabled) = options.text_normalization {
      body["text_normalization"] = json!(enabled);
    }
    if let Some(replacements) = &options.pronunciation {
      body["replace"] = json!(replacements);
    }
    ("/v1/tts".into(), body)
  })
}

impl Options {
  fn validate(&self) -> Result<(), Error> {
    let invalid = |text: &str| Error::Build(text.into());
    if let Some(output) = &self.speech_output {
      output.validate()?;
    }
    if let Some(replacements) = &self.pronunciation {
      if replacements.len() > 200 {
        return Err(invalid("pronunciation accepts at most 200 replacements"));
      }
      let mut phrases = std::collections::HashSet::new();
      for (key, value) in replacements {
        if key.trim().is_empty()
          || key.chars().count() > 100
          || key.chars().any(|c| !(c.is_alphanumeric() || c == '\'' || c == '’' || c == ' '))
        {
          return Err(invalid(
            "pronunciation keys must contain 1..100 letters, digits, apostrophes or spaces",
          ));
        }
        if value.chars().count() > 128 {
          return Err(invalid("pronunciation values must not exceed 128 characters"));
        }
        let normalized = key.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        if !phrases.insert(normalized) {
          return Err(invalid(
            "pronunciation contains duplicate phrases ignoring case and spacing",
          ));
        }
      }
    }
    if self.speed.is_some_and(|v| !v.is_finite() || !(0.7..=1.5).contains(&v)) {
      return Err(invalid("xAI speech speed must be 0.7..1.5"));
    }
    Ok(())
  }
}

pub fn timed_response(values: &crate::extensions::shared::parameters::Parameters) -> bool {
  values.get("with_timestamps").and_then(Value::as_bool) == Some(true)
}
