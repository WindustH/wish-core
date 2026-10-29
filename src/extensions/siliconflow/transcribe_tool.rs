//! Extension-owned model tool contract. Similar concepts use the same vocabulary.
use crate::extensions::catalog::{Invocation, Permission, Registration};
use serde_json::json;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
  pub audio: String,
  #[serde(default)]
  pub service: Option<String>,
}
fn specification() -> crate::protocol::Tool {
  crate::protocol::Tool {
    name: "siliconflow_transcribe".into(),
    description: "siliconflow transcription. Read speech from a session-owned audio or video blob."
      .into(),
    input_schema: json!({"type": "object", "additionalProperties": false, "properties": {"audio": {"type": "string", "description": "Session-owned audio/video blob ID."}, "service": {"type": "string", "description": "Configured service ID; omit to use configured priority for this extension."}}, "required": ["audio"]}),
  }
}
pub const REGISTRATION: Registration = Registration {
  extension: "siliconflow",
  name: "siliconflow_transcribe",
  summary: "siliconflow transcription",
  tags: &["audio", "asr", "transcription"],
  service_presets: &["siliconflow_asr"],
  accepts_model: |_| true,
  permission: Permission::Transcription,
  specification,
  examples: || json!([{"audio": "<blob ID>"}]),
  parse: |value| {
    serde_json::from_value(value).map(Invocation::SiliconflowTranscribe).map_err(|e| e.to_string())
  },
};

impl Request {
  pub(crate) fn validate_audio(
    &self,
    byte_len: u64,
    mime: &str,
  ) -> Result<(), crate::protocol::error::Error> {
    super::transcription::validate(byte_len, mime, None, false).map(|_| ())
  }
  pub(crate) fn audio_submission(
    &self,
    model: &str,
    bytes: &[u8],
    mime: &str,
  ) -> Result<(String, crate::protocol::endpoint::Draft), crate::protocol::error::Error> {
    super::transcription::submission(model, bytes, mime, None, false)
  }
}
