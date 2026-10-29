//! Extension-owned model tool contract. Similar concepts use the same vocabulary.
use crate::extensions::catalog::{Invocation, Permission, Registration};
use serde_json::json;
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
  prompt: String,
  #[serde(default)]
  service: Option<String>,
}
fn specification() -> crate::protocol::Tool {
  crate::protocol::Tool{name:"chatgpt_image".into(),description:"chatgpt_image generation. Uses only this extension's services; images are stored as session-owned blobs and their paths are returned. Do not repeat a submission with an unknown outcome.".into(),input_schema:json!({"type": "object", "additionalProperties": false, "properties": {"prompt": {"type": "string", "minLength": 1}, "service": {"type": "string", "description": "Configured service ID; omit to use configured priority for this extension."}}, "required": ["prompt"]})}
}
pub const REGISTRATION: Registration = Registration {
  extension: "openai",
  name: "chatgpt_image",
  summary: "chatgpt_image generation",
  tags: &["image", "generation"],
  service_presets: &["codex_images"],
  accepts_model: |_| true,
  permission: Permission::ImageGeneration,
  specification,
  examples: || json!([{"prompt": "A paper crane"}]),
  parse: |value| {
    let request: Request = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;

    serde_json::to_value(request).map(Invocation::Configured).map_err(|e| e.to_string())
  },
};
