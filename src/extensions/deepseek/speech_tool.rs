//! Supplier-owned TTS tool; durable jobs preserve outcomes without repeating paid requests.
use crate::extensions::{
  catalog::{Invocation, Permission, Registration},
  invocation::{Executable, Future, Host},
  media_runtime::{MediaKind, MediaRequest},
};
use serde_json::json;

#[derive(serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
  Run {
    text: String,
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    parameters: super::speech::Options,
  },
  Status {
    job_id: String,
  },
  ListJobs {},
}

fn specification() -> crate::protocol::Tool {
  crate::protocol::Tool {
    name: "deepseek_speech".into(),
    description: "Generate speech with DeepSeek and save a playable WAV in this session. text accepts at most 10000 billing characters (Han characters count twice). parameters.voice is stella (female, default) or tide (male); filter_markdown defaults to true. Language is detected automatically; speed and model selection are not supported. Sends complete text, not a live WebSocket session. status/list_jobs recover existing jobs without generating again; never repeat an outcome_unknown submission.".into(),
    input_schema: json!({"type":"object","additionalProperties":false,
      "properties":{
        "action":{"type":"string","enum":["run","status","list_jobs"],"default":"run"},
        "text":{"type":"string","minLength":1},
        "service":{"type":"string","description":"Configured service ID; omit to use this extension's priority."},
        "job_id":{"type":"string","minLength":1},
        "parameters":{"type":"object","additionalProperties":false,"properties":{
          "voice":{"type":"string","enum":["stella","tide"]},
          "filter_markdown":{"type":"boolean"}
        }}
      },
      "oneOf":[
        {"required":["text"],"properties":{"action":{"const":"run"}},"not":{"required":["job_id"]}},
        {"required":["action","job_id"],"properties":{"action":{"const":"status"}},"not":{"anyOf":[{"required":["text"]},{"required":["service"]},{"required":["parameters"]}]}},
        {"required":["action"],"properties":{"action":{"const":"list_jobs"}},"not":{"anyOf":[{"required":["text"]},{"required":["service"]},{"required":["parameters"]},{"required":["job_id"]}]}}
      ]
    }),
  }
}

pub const REGISTRATION: Registration = Registration {
  extension: "deepseek",
  name: "deepseek_speech",
  summary: "DeepSeek text to speech with Stella and Tide voices",
  tags: &["audio", "speech", "tts", "generation"],
  service_presets: &["deepseek_speech"],
  model_override: false,
  permission: Permission::AudioGeneration,
  specification,
  examples: || json!([{"text":"你好，欢迎使用 Wish。","parameters":{"voice":"stella"}}]),
  parse: |mut value| {
    let object = value.as_object_mut().ok_or("expected tool arguments object")?;
    object.entry("action").or_insert_with(|| json!("run"));
    let request: Request = serde_json::from_value(value).map_err(|e| e.to_string())?;
    match &request {
      Request::Run { text, .. } if text.trim().is_empty() => {
        return Err("text must not be empty".into());
      }
      Request::Status { job_id } if job_id.trim().is_empty() => {
        return Err("job_id must not be empty".into());
      }
      _ => {}
    }
    Ok(Invocation::new(request))
  },
};

impl Executable for Request {
  fn execute<'a>(self: Box<Self>, host: &'a dyn Host) -> Future<'a> {
    Box::pin(async move {
      let request = match *self {
        Request::Run { text, service, parameters } => {
          let options = serde_json::to_value(parameters).map_err(|e| e.to_string())?;
          MediaRequest {
            action: Some("run".into()),
            kind: Some(MediaKind::Speech),
            prompt: Some(text),
            provider: service,
            options: options.as_object().ok_or("expected speech options")?.clone(),
            ..Default::default()
          }
        }
        Request::Status { job_id } => {
          MediaRequest { action: Some("status".into()), job_id: Some(job_id), ..Default::default() }
        }
        Request::ListJobs {} => {
          MediaRequest { action: Some("list_jobs".into()), ..Default::default() }
        }
      };
      host.media(request).await
    })
  }
}
