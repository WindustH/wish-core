//! xAI transcription controls and multipart request.
use crate::protocol::{endpoint::Draft, error::Error};
use serde::Deserialize;
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
  pub format: Option<bool>,
  pub multichannel: Option<bool>,
  pub filler_words: Option<bool>,
  pub vad_threshold: Option<f64>,
  pub keyterms: Vec<String>,
}
impl Options {
  pub(crate) fn validate(&self, language: Option<&str>) -> Result<(), Error> {
    if self.format == Some(true) && language.is_none() {
      return Err(Error::Build("text formatting requires language".into()));
    }
    if self.vad_threshold.is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v)) {
      return Err(Error::Build("vad_threshold must be between 0 and 1".into()));
    }
    if self.keyterms.len() > 100
      || self.keyterms.iter().any(|term| {
        term.trim().is_empty() || term.chars().count() > 50 || term.chars().any(char::is_control)
      })
    {
      return Err(Error::Build(
        "keyterms accepts at most 100 nonempty terms of 50 characters each".into(),
      ));
    }
    Ok(())
  }
}

pub(crate) fn validate(
  byte_len: u64,
  mime: &str,
  language: Option<&str>,
  diarize: bool,
  options: &Options,
) -> Result<&'static str, Error> {
  options.validate(language)?;
  let _ = byte_len;
  let _ = diarize;
  if let Some(language) = language
    && !((2..=3).contains(&language.len()) && language.bytes().all(|c| c.is_ascii_alphabetic())
      || ["es-MX", "es-ES", "pt-BR", "pt-PT", "ar-EG", "ar-AE", "ar-SA"]
        .iter()
        .any(|v| v.eq_ignore_ascii_case(language)))
  {
    return Err(Error::Build("invalid transcription language code".into()));
  }
  crate::extensions::shared::multipart::audio_extension(mime)
}
pub(crate) fn submission(
  model: &str,
  bytes: &[u8],
  mime: &str,
  language: Option<&str>,
  diarize: bool,
  options: &Options,
) -> Result<(String, Draft), Error> {
  let extension = validate(bytes.len() as u64, mime, language, diarize, options)?;
  let mut fields = vec![("model", model.to_owned())];
  fields.push(("diarize", diarize.to_string()));
  for (name, value) in [
    ("format", options.format),
    ("multichannel", options.multichannel),
    ("filler_words", options.filler_words),
  ] {
    if let Some(value) = value {
      fields.push((name, value.to_string()));
    }
  }
  if let Some(value) = options.vad_threshold {
    fields.push(("vad_threshold", value.to_string()));
  }
  fields.extend(options.keyterms.iter().map(|term| ("keyterm", term.clone())));

  if let Some(language) = language {
    fields.push(("language", language.into()));
  }
  Ok((
    "/v1/stt".into(),
    crate::extensions::shared::multipart::file(&fields, bytes, mime, extension),
  ))
}

pub fn validate_configuration(
  model: &str,
  values: &crate::extensions::shared::parameters::Parameters,
) -> Result<(), Error> {
  if values.values().any(|value| !value.is_null()) {
    return Err(Error::Build("ASR defaults are specified per tool call".into()));
  }
  if model != "grok-voice-transcribe-2.0" {
    return Err(Error::Build("unsupported xAI ASR model".into()));
  }
  Ok(())
}

/// Tool-facing transcript summary; the host separately publishes the complete response blob.
pub(crate) fn result(bytes: &[u8]) -> Result<serde_json::Value, String> {
  let output: serde_json::Value =
    serde_json::from_slice(bytes).map_err(|_| "invalid xai transcription reply")?;
  let text = output["text"].as_str().ok_or("xai transcription reply has no transcript")?;
  Ok(serde_json::json!({
    "language":output.get("language"),
    "text":text.chars().take(32_000).collect::<String>(),
    "truncated":text.chars().count()>32_000,
    "format":"json",
    "word_timestamps":output["words"].as_array().is_some_and(|words| !words.is_empty())
  }))
}
