//! Openai speech tool contract; tags are discovery metadata only.
use crate::extensions::catalog::{Invocation, Permission, Registration};
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
    name: "openai_speech".into(),
    description: "openai_speech: run submits one job; status and list_jobs recover existing jobs without resubmitting. Inspect configured services first. Outputs are saved as session-owned blobs.".into(),
    input_schema: json!({"type":"object","additionalProperties":false,
      "properties":{
        "action":{"type":"string","enum":["run","status","list_jobs"],"default":"run"},
        "text":{"type":"string","minLength":1},
        "service":{"type":"string","description":"Configured service ID; omit to use this extension's priority."},
        "job_id":{"type":"string","minLength":1},
        "parameters":{"type":"object","additionalProperties":false,"properties":{"voice":{"type":"string"}}}
      },
      "oneOf":[
        {"required":["text"],"properties":{"action":{"const":"run"}},"not":{"required":["job_id"]}},
        {"required":["action","job_id"],"properties":{"action":{"const":"status"}},"not":{"anyOf":[{"required":["text"]},{"required":["service"]},{"required":["parameters"]}]}},
        {"required":["action"],"properties":{"action":{"const":"list_jobs"}},"not":{"anyOf":[{"required":["text"]},{"required":["service"]},{"required":["parameters"]},{"required":["job_id"]}]}}
      ]
    })
  }
}
pub const REGISTRATION: Registration = Registration {
  extension: "openai",
  name: "openai_speech",
  summary: "openai_speech",
  tags: &["audio", "speech"],
  service_presets: &["openai_speech", "siliconflow_speech"],
  accepts_model: |_| true,
  permission: Permission::AudioGeneration,
  specification,
  examples: || json!([{"text": "Hello world"}]),
  parse: |mut value| {
    let object = value.as_object_mut().ok_or("expected tool arguments object")?;
    object.entry("action").or_insert_with(|| json!("run"));
    let request: Request = serde_json::from_value::<Request>(value).map_err(|e| e.to_string())?;
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
