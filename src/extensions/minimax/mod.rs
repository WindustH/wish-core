//! MiniMax speech has a native text/voice contract and its own recovery operations.
use crate::{
  extensions::catalog::{Invocation, Permission, Registration},
  protocol::Tool,
};
pub mod parameters;
pub mod speech;
use serde::Deserialize;
use serde_json::json;
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Voice {
  pub id: Option<String>,
  pub speed: Option<f64>,
  pub volume: Option<f64>,
  pub pitch: Option<i32>,
  pub emotion: Option<String>,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Speech {
  Run {
    text: String,
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    voice: Voice,
    #[serde(default)]
    parameters: Box<parameters::Parameters>,
  },
  Status {
    job_id: String,
  },
  ListJobs {},
}
pub fn specification() -> Tool {
  Tool {
    name:"minimax_speech".into(),
    description:"Synthesize speech with MiniMax. run reads text exactly; fewer than 10000 characters. voice.id selects a MiniMax voice; parameters.timbre_weights mixes up to four voices (weights 1..100, no sum requirement), omit voice.id when mixing; omitted voice fields inherit service settings. speed 0.5..2, volume >0..10, pitch -12..12. fluent/whisper emotions require a supporting model. Omit service to use the first compatible MiniMax speech service in configured priority order. Output is a durable session job; files appear as local Wish blob paths after download. Use status with job_id to finish/resume downloads, including after restart; list_jobs recovers this extension's session jobs. Status never resubmits generation. If outcome_unknown, do not automatically repeat the paid request. parameters provides native language, pronunciation dictionary, audio encoding and voice effects. subtitle_enable saves sentence or word subtitle JSON in a separate Wish blob. Raw audio and voice cloning are not yet exposed by this tool.".into(),
    input_schema:json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{
      "action":{"type":"string","enum":["run","status","list_jobs"]},
      "text":{"type":"string","minLength":1,"maxLength":9999,"description":"Required for run."},
      "service":{"type":"string","description":"Optional configured MiniMax speech service ID; run only."},
      "parameters":parameters::schema(),
      "voice":{"type":"object","additionalProperties":false,"properties":{
        "id":{"type":"string"},"speed":{"type":"number","minimum":0.5,"maximum":2},
        "volume":{"type":"number","exclusiveMinimum":0,"maximum":10},
        "pitch":{"type":"integer","minimum":-12,"maximum":12},
        "emotion":{"type":"string","enum":["happy","sad","angry","fearful","disgusted","surprised","calm","fluent","whisper"]}
      }},
      "job_id":{"type":"string","description":"Required for status; no generation fields allowed. list_jobs accepts only action."}
    }})
  }
}
pub const SPEECH: Registration = Registration {
  extension: "minimax",
  name: "minimax_speech",
  summary: "MiniMax speech synthesis with voice controls and durable download recovery",
  tags: &[
    "audio",
    "speech",
    "tts",
    "generation",
    "subtitles",
    "pronunciation",
    "voice-effects",
    "voice-mixing",
  ],
  service_presets: &["minimax_speech"],
  accepts_model: |_| true,
  permission: Permission::AudioGeneration,
  specification,
  examples: || {
    json!([
      {"action":"run","text":"Hello, welcome to Wish.","voice":{"id":"English_Graceful_Lady","speed":1.1}},
      {"action":"run","text":"Welcome to Wish.","parameters":{"timbre_weights":[{"voice_id":"English_Graceful_Lady","weight":30},{"voice_id":"English_Insightful_Speaker","weight":70}]}},
      {"action":"status","job_id":"<returned job ID>"},
      {"action":"list_jobs"}
    ])
  },
  parse: |value| {
    serde_json::from_value::<Speech>(value).map(Invocation::new).map_err(|e| e.to_string())
  },
};
pub mod endpoint;
pub(crate) mod search;
pub(crate) mod search_tool;
pub mod sources;
pub(crate) mod video;
pub mod video_contract;
pub mod video_status;
pub(crate) mod video_tool;

pub(crate) mod minimax_h3_driver;

pub(crate) mod minimax_speech_driver;

pub(crate) mod execution;

pub(crate) mod package;
