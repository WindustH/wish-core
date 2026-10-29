//! Provider-owned transcription validation and fields.
use crate::protocol::{endpoint::Draft, error::Error};

pub(crate) fn validate(
  byte_len: u64,
  mime: &str,
  language: Option<&str>,
  diarize: bool,
) -> Result<&'static str, Error> {
  let _ = byte_len;
  if diarize {
    return Err(Error::Build(
      "this transcription protocol does not support speaker diarization".into(),
    ));
  }
  if language.is_some() {
    return Err(Error::Build("SiliconFlow ASR does not support language hints".into()));
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
  let mut fields = vec![("model", model.to_owned())];
  if let Some(language) = language {
    fields.push(("language", language.into()));
  }
  Ok((
    "/v1/audio/transcriptions".into(),
    crate::extensions::shared::multipart::file(&fields, bytes, mime, extension),
  ))
}

pub fn validate_configuration(
  _model: &str,
  values: &crate::extensions::shared::parameters::Parameters,
) -> Result<(), Error> {
  if values.values().any(|value| !value.is_null()) {
    return Err(Error::Build("ASR defaults are specified per tool call".into()));
  }
  Ok(())
}

/// Tool-facing transcript summary; the host separately publishes the complete response blob.
pub(crate) fn result(bytes: &[u8]) -> Result<serde_json::Value, String> {
  let output: serde_json::Value =
    serde_json::from_slice(bytes).map_err(|_| "invalid siliconflow transcription reply")?;
  let text = output["text"].as_str().ok_or("siliconflow transcription reply has no transcript")?;
  Ok(serde_json::json!({
    "language":output.get("language"),
    "text":text.chars().take(32_000).collect::<String>(),
    "truncated":text.chars().count()>32_000,
    "format":"json",
    "word_timestamps":output["words"].as_array().is_some_and(|words| !words.is_empty())
  }))
}
