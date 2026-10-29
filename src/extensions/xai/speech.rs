//! Native xAI speech requests. These names mirror the speech API, not other suppliers' controls.
use crate::{
  extensions::catalog::{Invocation, Permission, Registration},
  protocol::Tool,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OutputFormat {
  pub codec: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub sample_rate: Option<u32>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub bit_rate: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
  pub text: String,
  pub service: Option<String>,
  pub voice_id: Option<String>,
  pub language: Option<String>,
  pub speed: Option<f64>,
  pub output_format: Option<OutputFormat>,
  pub with_timestamps: Option<bool>,
  pub text_normalization: Option<bool>,
  pub replace: Option<std::collections::BTreeMap<String, String>>,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
  Run(Box<Run>),
  Status { job_id: String },
  ListJobs {},
}
pub fn specification() -> Tool {
  Tool {
    name:"xai_speech".into(),
    description:"Generate speech with xAI and store audio in this session's Wish blobs. run reads text (up to 60000 characters), including inline speech tags. Omit service to select the first compatible configured xAI speech service. Omitted settings inherit service defaults; voice_id ultimately defaults to eve and language to auto. speed is 0.7..1.5. output_format supports stored MP3 or WAV, not raw/telephony encodings. with_timestamps stores character timing JSON as a separate blob; timing reflects spoken replacements. replace is a complete per-call pronunciation dictionary override (respelling or IPA); {} clears configured replacements. At most200 phrases, keys1..100 characters (letters, digits, apostrophes, spaces), values up to128; case/spacing duplicates rejected. status resumes this extension's session jobs without regenerating; list_jobs recovers IDs after interruption. Never automatically repeat outcome_unknown paid submissions. This tool does not clone voices or offer realtime streaming.".into(),
    input_schema:json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{
      "action":{"type":"string","enum":["run","status","list_jobs"]},
      "text":{"type":"string","minLength":1,"maxLength":60000,"description":"Required for run."},
      "service":{"type":"string"},"voice_id":{"type":"string"},
      "language":{"type":"string","description":"BCP-47 code or auto."},
      "speed":{"type":"number","minimum":0.7,"maximum":1.5},
      "output_format":{"type":"object","additionalProperties":false,"required":["codec"],"properties":{
        "codec":{"type":"string","enum":["mp3","wav"]},
        "sample_rate":{"type":"integer","enum":[8000,16000,22050,24000,44100,48000]},
        "bit_rate":{"type":"integer","enum":[32000,64000,96000,128000,192000],"description":"MP3 only."}
      }},
      "with_timestamps":{"type":"boolean"},"text_normalization":{"type":"boolean"},
      "replace":{"type":"object","maxProperties":200,"additionalProperties":{"type":"string","maxLength":128}},
      "job_id":{"type":"string","description":"Required for status. status accepts only action/job_id; list_jobs accepts only action."}
    }})
  }
}
pub const REGISTRATION: Registration = Registration {
  extension: "xai",
  name: "xai_speech",
  summary: "xAI speech synthesis with pronunciation, output formats and character timing",
  tags: &["audio", "speech", "tts", "generation", "timestamps"],
  service_presets: &["xai_speech"],
  accepts_model: |_| true,
  permission: Permission::AudioGeneration,
  specification,
  examples: || {
    json!([
      {"action":"run","text":"Hello Wish","voice_id":"eve","language":"en","output_format":{"codec":"wav","sample_rate":24000},"with_timestamps":true},
      {"action":"status","job_id":"<returned job ID>"},{"action":"list_jobs"}
    ])
  },
  parse: |value| {
    serde_json::from_value(value).map(Invocation::XaiSpeech).map_err(|error| error.to_string())
  },
};

impl Run {
  /// Translate this tool's names to xAI service-default fields inside the extension.
  pub fn parameters(&self) -> serde_json::Map<String, serde_json::Value> {
    serde_json::Map::from_iter(
      [
        ("voice".into(), json!(self.voice_id)),
        ("speed".into(), json!(self.speed)),
        ("language".into(), json!(self.language)),
        ("speech_output".into(), json!(self.output_format)),
        ("with_timestamps".into(), json!(self.with_timestamps)),
        ("text_normalization".into(), json!(self.text_normalization)),
        ("pronunciation".into(), json!(self.replace)),
      ]
      .into_iter()
      .filter(|(_, value)| !value.is_null()),
    )
  }
}
