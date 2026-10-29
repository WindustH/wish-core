//! Extension-owned model tool contract. Similar concepts use the same vocabulary.
use crate::extensions::catalog::{Invocation, Permission, Registration};
use serde_json::json;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
  pub audio: String,
  #[serde(default)]
  pub service: Option<String>,
  #[serde(default)]
  pub language: Option<String>,
}
fn specification() -> crate::protocol::Tool {
  crate::protocol::Tool {
    name: "openai_transcribe".into(),
    description: "openai transcription. Read speech from a session-owned audio or video blob."
      .into(),
    input_schema: json!({"type": "object", "additionalProperties": false, "properties": {"audio": {"type": "string", "description": "Session-owned audio/video blob ID."}, "service": {"type": "string", "description": "Configured service ID; omit to use configured priority for this extension."}, "language": {"type": "string"}}, "required": ["audio"]}),
  }
}
pub const REGISTRATION: Registration = Registration {
  extension: "openai",
  name: "openai_transcribe",
  summary: "openai transcription",
  tags: &["audio", "asr", "transcription"],
  service_presets: &["openai_asr", "groq_asr"],
  accepts_model: |_| true,
  permission: Permission::Transcription,
  specification,
  examples: || json!([{"audio": "<blob ID>"}]),
  parse: |value| {
    serde_json::from_value::<Request>(value).map(Invocation::new).map_err(|e| e.to_string())
  },
};

impl Request {
  pub(crate) fn validate_audio(
    &self,
    byte_len: u64,
    mime: &str,
  ) -> Result<(), crate::protocol::error::Error> {
    super::transcription::validate(byte_len, mime, self.language.as_deref(), false).map(|_| ())
  }
  pub(crate) fn audio_submission(
    &self,
    model: &str,
    bytes: &[u8],
    mime: &str,
  ) -> Result<(String, crate::protocol::endpoint::Draft), crate::protocol::error::Error> {
    super::transcription::submission(model, bytes, mime, self.language.as_deref(), false)
  }
}
