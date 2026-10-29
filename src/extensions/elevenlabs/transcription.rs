//! Provider-owned transcription validation and fields.
use crate::protocol::{endpoint::Draft, error::Error};

pub(crate) fn validate(
  byte_len: u64,
  mime: &str,
  language: Option<&str>,
  diarize: bool,
) -> Result<&'static str, Error> {
  let _ = byte_len;
  let _ = diarize;
  if let Some(language) = language
    && !((2..=3).contains(&language.len()) && language.bytes().all(|c| c.is_ascii_alphabetic()))
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
) -> Result<(String, Draft), Error> {
  let extension = validate(bytes.len() as u64, mime, language, diarize)?;
  let mut fields = vec![
    ("model_id", model.to_owned()),
    ("diarize", diarize.to_string()),
    ("timestamps_granularity", "word".into()),
  ];
  if let Some(language) = language {
    fields.push(("language_code", language.into()));
  }
  Ok((
    "/v1/speech-to-text".into(),
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
  if !matches!(model, "scribe_v1" | "scribe_v2") {
    return Err(Error::Build("unsupported ASR model".into()));
  }
  Ok(())
}

/// Tool-facing transcript summary; the host separately publishes the complete response blob.
pub(crate) fn result(bytes: &[u8]) -> Result<serde_json::Value, String> {
  let output: serde_json::Value =
    serde_json::from_slice(bytes).map_err(|_| "invalid elevenlabs transcription reply")?;
  let text = output["text"].as_str().ok_or("elevenlabs transcription reply has no transcript")?;
  Ok(serde_json::json!({
    "language":output.get("language_code"),
    "text":text.chars().take(32_000).collect::<String>(),
    "truncated":text.chars().count()>32_000,
    "format":"json",
    "word_timestamps":output["words"].as_array().is_some_and(|words| !words.is_empty())
  }))
}
