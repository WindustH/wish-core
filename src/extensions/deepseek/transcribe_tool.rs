use crate::extensions::{
  catalog::{Invocation, Permission, Registration},
  invocation::{Executable, Future, Host},
  transcription_runtime::Transcription,
};
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
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
    name: "deepseek_transcribe".into(),
    description: "Transcribe a session-owned audio blob using DeepSeek. Accepts 16-bit PCM WAV (8000/16000/22050/24000/32000/44100/48000 Hz) or Ogg Opus, mono or stereo, at most 8 MiB and 120 seconds. Stereo uses only the left channel. Language is detected automatically; no model/language override or streaming. Codec and duration are checked by the service. Returns transcript text, duration_ms, and the complete JSON response as a session blob. Paid requests are never automatically repeated or sent to another service.".into(),
    input_schema: json!({"type":"object","additionalProperties":false,"required":["audio"],"properties":{
      "audio":{"type":"string","description":"Session-owned audio blob ID."},
      "service":{"type":"string","description":"Configured service ID; omit to use this extension's priority."}
    }}),
  }
}

pub const REGISTRATION: Registration = Registration {
  extension: "deepseek",
  name: "deepseek_transcribe",
  summary: "DeepSeek WAV/Opus file transcription",
  tags: &["audio", "asr", "transcription", "speech-to-text"],
  service_presets: &["deepseek_asr"],
  model_override: false,
  permission: Permission::Transcription,
  specification,
  examples: || json!([{"audio":"<session audio blob ID>"}]),
  parse: |value| {
    serde_json::from_value::<Request>(value).map(Invocation::new).map_err(|e| e.to_string())
  },
};

impl Transcription for Request {
  fn registration(&self) -> &'static Registration {
    &REGISTRATION
  }
  fn credentials_ready(&self, c: &Credentials) -> bool {
    super::endpoint::ready(c)
  }
  fn call(&self, base: &str, path: &str, c: &Credentials, d: Draft) -> Result<Call, Error> {
    super::endpoint::call(base, path, c, d)
  }
  fn result(&self, bytes: &[u8]) -> Result<serde_json::Value, String> {
    super::transcription::result(bytes)
  }
  fn blob_id(&self) -> &str {
    &self.audio
  }
  fn service(&self) -> Option<&str> {
    self.service.as_deref()
  }
  fn validate_audio(&self, byte_len: u64, mime: &str) -> Result<(), Error> {
    super::transcription::validate(byte_len, mime).map(|_| ())
  }
  fn submission(&self, _model: &str, bytes: &[u8], mime: &str) -> Result<(String, Draft), Error> {
    super::transcription::submission(bytes, mime)
  }
}

impl Executable for Request {
  fn execute<'a>(self: Box<Self>, host: &'a dyn Host) -> Future<'a> {
    Box::pin(async move { host.transcribe(self).await })
  }
}
