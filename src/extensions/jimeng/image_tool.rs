//! Extension-owned model tool contract. Similar concepts use the same vocabulary.
use crate::extensions::catalog::{Invocation, Permission, Registration};
use serde_json::{Value, json};
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
  prompt: String,
  #[serde(default)]
  service: Option<String>,
  #[serde(default)]
  parameters: serde_json::Map<String, Value>,
  #[serde(default)]
  reference_images: Vec<String>,
}
fn specification() -> crate::protocol::Tool {
  crate::protocol::Tool{name:"jimeng_image".into(),description:"jimeng_image generation. Uses only this extension's services; images are stored as session-owned blobs and their paths are returned. Do not repeat a submission with an unknown outcome.".into(),input_schema:json!({"type": "object", "additionalProperties": false, "properties": {"prompt": {"type": "string", "minLength": 1}, "service": {"type": "string", "description": "Configured service ID; omit to use configured priority for this extension."}, "parameters": {"type": "object", "description": "Provider-specific overrides. Unsupported parameters are rejected before submission."}, "reference_images": {"type": "array", "maxItems": 14, "items": {"type": "string"}, "description": "Session-owned image blob IDs or public HTTPS URLs when supported by the model."}}, "required": ["prompt"]})}
}
pub const REGISTRATION: Registration = Registration {
  extension: "jimeng",
  name: "jimeng_image",
  summary: "jimeng_image generation",
  tags: &["image", "generation", "editing", "reference"],
  service_presets: &["jimeng_images"],
  accepts_model: |_| true,
  permission: Permission::ImageGeneration,
  specification,
  examples: || json!([{"prompt": "A paper crane"}]),
  parse: |value| {
    let request: Request = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;

    serde_json::to_value(request).map(Invocation::Configured).map_err(|e| e.to_string())
  },
};
