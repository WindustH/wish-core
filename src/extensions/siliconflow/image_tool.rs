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
}
fn specification() -> crate::protocol::Tool {
  crate::protocol::Tool{name:"siliconflow_image".into(),description:"siliconflow_image generation. Uses only this extension's services; images are stored as session-owned blobs and their paths are returned. Do not repeat a submission with an unknown outcome.".into(),input_schema:json!({"type": "object", "additionalProperties": false, "properties": {"prompt": {"type": "string", "minLength": 1}, "service": {"type": "string", "description": "Configured service ID; omit to use configured priority for this extension."}, "parameters": {"type": "object", "description": "Provider-specific overrides. Unsupported parameters are rejected before submission."}}, "required": ["prompt"]})}
}
pub const REGISTRATION: Registration = Registration {
  extension: "siliconflow",
  name: "siliconflow_image",
  summary: "siliconflow_image generation",
  tags: &["image", "generation"],
  service_presets: &["siliconflow_images"],
  accepts_model: |_| true,
  permission: Permission::ImageGeneration,
  specification,
  examples: || json!([{"prompt": "A paper crane"}]),
  parse: |value| {
    let request: Request = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;

    Ok(Invocation::new(request))
  },
};

impl crate::extensions::invocation::Executable for Request {
  fn execute<'a>(
    self: Box<Self>,
    host: &'a dyn crate::extensions::invocation::Host,
  ) -> crate::extensions::invocation::Future<'a> {
    Box::pin(async move {
      host
        .image(crate::extensions::image_runtime::ImageRequest {
          prompt: self.prompt,
          provider: self.service,
          options: self.parameters,
          images: Vec::new(),
          mask: None,
          markup: None,
          references: Vec::new(),
        })
        .await
    })
  }
}
