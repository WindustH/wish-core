//! Supplier image wire format.
// Local resource bound, not a claim about a particular model's upstream entitlement.
pub(crate) const REFERENCE_LIMIT: usize = 14;

pub(crate) fn generation(
  model: &str,
  prompt: &str,
  parameters: &serde_json::Map<String, serde_json::Value>,
) -> Result<crate::protocol::endpoint::Draft, crate::protocol::error::Error> {
  let mut args = parameters.clone();
  args.insert("prompt".into(), serde_json::json!(prompt));
  args.insert("model".into(), serde_json::json!(model));
  crate::protocol::endpoint::Draft::post_json(&serde_json::Value::Object(args))
}

pub(crate) fn decode(
  body: &serde_json::Value,
) -> Result<crate::extensions::shared::images::ImageReply, crate::protocol::error::Error> {
  super::image::decode(body)
}

/// Codex ImagesClient::edit sends JSON ImageReference::Inline values, not multipart.
/// See openai/codex codex-rs/codex-api/src/endpoint/images.rs.
pub(crate) fn edit(
  model: &str,
  prompt: &str,
  parameters: &serde_json::Map<String, serde_json::Value>,
  images: &[crate::extensions::shared::images::ReferenceInput],
) -> Result<crate::protocol::endpoint::Draft, crate::protocol::error::Error> {
  use crate::extensions::shared::images::ReferenceInput;
  use crate::protocol::{endpoint::Draft, error::Error};
  use serde_json::{Value, json};
  if images.is_empty() || images.len() > REFERENCE_LIMIT {
    return Err(Error::Build(
      "ChatGPT image edits require between 1 and 14 reference images".into(),
    ));
  }
  let images = images
    .iter()
    .map(|image| match image {
      ReferenceInput::Inline(image) => {
        Ok(json!({"image_url":format!("data:{};base64,{}",image.mime,image.data)}))
      }
      ReferenceInput::Url(_) => {
        Err(Error::Build("ChatGPT image edits require session-owned image blobs".into()))
      }
    })
    .collect::<Result<Vec<_>, _>>()?;
  let mut args = parameters.clone();
  args.insert("model".into(), json!(model));
  args.insert("prompt".into(), json!(prompt));
  args.insert("images".into(), json!(images));
  let mut draft = Draft::post_json(&Value::Object(args))?;
  draft.path = Some("/images/edits".into());
  Ok(draft)
}
