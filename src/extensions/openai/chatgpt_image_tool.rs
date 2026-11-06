//! Extension-owned model tool contract. Similar concepts use the same vocabulary.
use crate::extensions::catalog::{Invocation, Permission, Registration};
use serde_json::json;
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
  prompt: String,
  #[serde(default)]
  reference_images: Vec<String>,
  #[serde(default)]
  service: Option<String>,
}
fn specification() -> crate::protocol::Tool {
  crate::protocol::Tool{name:"chatgpt_image".into(),description:"Generate or edit images with ChatGPT/Codex. For edits, pass the original image blob IDs in reference_images; mentioning local file paths in the prompt does not upload images. Uses only this extension's services; images are stored as session-owned blobs and their paths are returned. Do not repeat a submission with an unknown outcome.".into(),input_schema:json!({"type": "object", "additionalProperties": false, "properties": {"prompt": {"type": "string", "minLength": 1}, "reference_images": {"type":"array", "maxItems":14, "items":{"type":"string"}, "description":"Session-owned image blob IDs to upload as reference images. Required when editing an existing image; local file paths and public URLs are not accepted."}, "service": {"type": "string", "description": "Configured service ID; omit to use configured priority for this extension."}}, "required": ["prompt"]})}
}
pub const REGISTRATION: Registration = Registration {
  extension: "openai",
  name: "chatgpt_image",
  summary: "ChatGPT image generation and reference editing",
  tags: &["image", "generation"],
  service_presets: &["codex_images"],
  model_override: true,
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
          model: None,
          prompt: self.prompt,
          provider: self.service,
          options: Default::default(),
          images: self.reference_images,
          mask: None,
          markup: None,
          references: Vec::new(),
        })
        .await
    })
  }
}
