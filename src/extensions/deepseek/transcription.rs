//! File ASR uses only a multipart file, with no model or language fields.
use crate::protocol::{endpoint::Draft, error::Error};

pub fn validate(byte_len: u64, mime: &str) -> Result<&'static str, Error> {
  if byte_len == 0 || byte_len > 8 * 1024 * 1024 {
    return Err(Error::Build("DeepSeek ASR requires a non-empty file of at most 8 MiB".into()));
  }
  match mime {
    "audio/wav" => Ok("wav"),
    "audio/ogg" => Ok("ogg"),
    _ => Err(Error::Build(
      "DeepSeek ASR accepts only 16-bit PCM WAV or Ogg Opus, at most 120 seconds".into(),
    )),
  }
}

pub fn submission(bytes: &[u8], mime: &str) -> Result<(String, Draft), Error> {
  let extension = validate(bytes.len() as u64, mime)?;
  Ok((
    "/v1/tools/asr".into(),
    crate::extensions::shared::multipart::file(&[], bytes, mime, extension),
  ))
}

pub fn result(bytes: &[u8]) -> Result<serde_json::Value, String> {
  let output: serde_json::Value =
    serde_json::from_slice(bytes).map_err(|_| "invalid DeepSeek transcription reply")?;
  let text = output["text"].as_str().ok_or("DeepSeek transcription reply has no transcript")?;
  let duration =
    output["duration_ms"].as_u64().ok_or("DeepSeek transcription reply has no duration_ms")?;
  Ok(serde_json::json!({
    "text":text.chars().take(32_000).collect::<String>(),
    "truncated":text.chars().count() > 32_000,
    "duration_ms":duration,
    "request_id":output.get("request_id"),
    "format":"json",
    "word_timestamps":false,
  }))
}
