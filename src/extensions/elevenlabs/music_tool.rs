//! Elevenlabs music tool contract; tags are discovery metadata only.
use crate::extensions::catalog::{Invocation, Permission, Registration};
use serde_json::json;
#[derive(serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
  Run {
    prompt: String,
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    parameters: super::music::Options,
  },
  Status {
    job_id: String,
  },
  ListJobs {},
}
fn specification() -> crate::protocol::Tool {
  crate::protocol::Tool {
    name: "elevenlabs_music".into(),
    description: "elevenlabs_music: run submits one job; status and list_jobs recover existing jobs without resubmitting. Inspect configured services first. Outputs are saved as session-owned blobs.".into(),
    input_schema: json!({"type":"object","additionalProperties":false,
      "properties":{
        "action":{"type":"string","enum":["run","status","list_jobs"],"default":"run"},
        "prompt":{"type":"string","minLength":1},
        "service":{"type":"string","description":"Configured service ID; omit to use this extension's priority."},
        "job_id":{"type":"string","minLength":1},
        "parameters":{"type":"object","additionalProperties":false,"properties":{"duration":{"type":"number","minimum":3,"maximum":600},"instrumental":{"type":"boolean"}}}
      },
      "oneOf":[
        {"required":["prompt"],"properties":{"action":{"const":"run"}},"not":{"required":["job_id"]}},
        {"required":["action","job_id"],"properties":{"action":{"const":"status"}},"not":{"anyOf":[{"required":["prompt"]},{"required":["service"]},{"required":["parameters"]}]}},
        {"required":["action"],"properties":{"action":{"const":"list_jobs"}},"not":{"anyOf":[{"required":["prompt"]},{"required":["service"]},{"required":["parameters"]},{"required":["job_id"]}]}}
      ]
    })
  }
}
pub const REGISTRATION: Registration = Registration {
  extension: "elevenlabs",
  name: "elevenlabs_music",
  summary: "elevenlabs_music",
  tags: &["audio", "music"],
  service_presets: &["elevenlabs_music"],
  accepts_model: |_| true,
  permission: Permission::AudioGeneration,
  specification,
  examples: || json!([{"prompt": "Soft rain in a forest"}]),
  parse: |mut value| {
    let object = value.as_object_mut().ok_or("expected tool arguments object")?;
    object.entry("action").or_insert_with(|| json!("run"));
    let request: Request = serde_json::from_value(value).map_err(|e| e.to_string())?;
    match &request {
      Request::Run { prompt, .. } if prompt.trim().is_empty() => {
        return Err("prompt must not be empty".into());
      }
      Request::Status { job_id } if job_id.trim().is_empty() => {
        return Err("job_id must not be empty".into());
      }
      _ => {}
    }
    Ok(Invocation::ElevenlabsMusic(request))
  },
};
