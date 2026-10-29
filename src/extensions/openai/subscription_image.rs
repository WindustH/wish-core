//! Supplier image wire format.

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
