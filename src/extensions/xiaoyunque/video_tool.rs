//! Extension-owned model tool contract. Similar concepts use the same vocabulary.
use crate::extensions::catalog::{Invocation, Permission, Registration};
use serde_json::json;
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
  #[serde(default = "run_action")]
  pub action: String,
  #[serde(default)]
  pub prompt: Option<String>,
  #[serde(default)]
  pub service: Option<String>,
  #[serde(default)]
  pub job_id: Option<String>,
  #[serde(default)]
  pub parameters: crate::extensions::xiaoyunque::video_options::VideoOptions,
  #[serde(default)]
  pub mode: Option<crate::extensions::shared::video::VideoMode>,
  #[serde(default)]
  pub inputs: Vec<crate::extensions::shared::video::VideoInput>,
}
fn run_action() -> String {
  "run".into()
}
fn specification() -> crate::protocol::Tool {
  crate::protocol::Tool{name:"xiaoyunque_video".into(),description:"xiaoyunque_video. run submits one job; status and list_jobs recover existing jobs without resubmitting. Inspect services and model capabilities first. Outputs are stored as session-owned blobs.".into(),input_schema:json!({"type": "object", "additionalProperties": false, "properties": {"action": {"type": "string", "enum": ["run", "status", "list_jobs"]}, "prompt": {"type": "string"}, "service": {"type": "string", "description": "Configured service ID; omit to use configured priority for this extension."}, "job_id": {"type": "string"}, "parameters": {"type": "object", "additionalProperties": false, "properties": {"duration": {"type": "number"}, "aspect_ratio": {"type": "string"}, "resolution": {"type": "string"}, "duration_hint": {"type": "string"}, "language": {"type": "string"}, "watermark": {"type": "boolean"}}}, "mode": {"type": "string", "enum": ["text", "frames", "reference", "edit", "extend"]}, "inputs": {"type": "array", "maxItems": 50, "items": {"type": "object", "properties": {"role": {"type": "string", "enum": ["first_frame", "last_frame", "reference_image", "reference_video", "reference_audio", "source_video"]}, "blob_id": {"type": "string"}, "url": {"type": "string"}, "job_id": {"type": "string"}}, "required": ["role"], "additionalProperties": false}}}})}
}
pub const REGISTRATION: Registration = Registration {
  extension: "xiaoyunque",
  name: "xiaoyunque_video",
  summary: "xiaoyunque_video",
  tags: &["video", "generation"],
  service_presets: &["xiaoyunque_video"],
  accepts_model: |_| true,
  permission: Permission::VideoGeneration,
  specification,
  examples: || json!([{"prompt": "A quiet scene"}]),
  parse: |value| {
    let request: Request = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
    match request.action.as_str() {
      "run" => {
        if request.job_id.is_some() {
          return Err("job_id is for status queries".into());
        }
      }
      "status" => {
        if request.job_id.is_none()
          || value.as_object().is_some_and(|v| v.keys().any(|k| k != "action" && k != "job_id"))
        {
          return Err("status requires only action and job_id".into());
        }
      }
      "list_jobs" => {
        if value.as_object().is_some_and(|v| v.len() != 1) {
          return Err("list_jobs accepts only action".into());
        }
      }
      _ => return Err("invalid action".into()),
    };
    Ok(Invocation::XiaoyunqueVideo(request))
  },
};
