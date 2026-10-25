//! Projects stored images for a model call without changing the original context.
use crate::{
  Error,
  protocol::{ContentBlock, Message},
  storage::blobs,
};
use base64::Engine;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};
/// Replaces each image with a reference to its blob, followed by the image itself unless the
/// model's declared capabilities or the upstream refuse images. With `save`, the images are stored
/// as blobs. Returns whether an image is still sent.
pub fn project_images(
  models: &BTreeMap<String, Value>,
  model: &str,
  image_dir: &Path,
  rejected: bool,
  messages: &mut [Message],
  save: bool,
) -> Result<bool, Error> {
  let unsupported = models
    .get(model)
    .and_then(|value| value["input_modalities"].as_array())
    .is_some_and(|values| !values.iter().any(|value| value == "image"));
  let mut images = false;
  for message in messages {
    let content = match message {
      Message::User { content, .. }
      | Message::Assistant { content, .. }
      | Message::System { content, .. }
      | Message::Developer { content, .. } => content,
      _ => continue,
    };
    let mut projected = Vec::new();
    for block in std::mem::take(content) {
      if let ContentBlock::Image { ref data_base64, .. } = block {
        images = true;
        let bytes = base64::engine::general_purpose::STANDARD
          .decode(data_base64)
          .map_err(|error| Error::Build(format!("invalid stored image: {error}")))?;
        let image_id = blobs::blob_id(&bytes);
        let path = image_dir.join(&image_id);
        if save {
          blobs::save_image(image_dir, data_base64)
            .map_err(|error| Error::Build(error.to_string()))?;
        }
        let notice = if unsupported {
          "The selected model's declared capabilities do not support image input. Only this file reference is sent; do not claim to have viewed the image. Use shell for file processing."
        } else if rejected {
          "The upstream rejected image input. Only this file reference is sent; do not claim to have viewed the image. Use shell for file processing."
        } else {
          "The image is included below. Do not call view_image again merely to confirm an image you have already viewed; use shell when further processing is needed."
        };
        projected.push(ContentBlock::Text {
          text: format!(
            "[Image sha256:{image_id}]\n{notice}\nSession-local blob path: {}",
            serde_json::json!(path)
          ),
        });
        if !unsupported && !rejected {
          projected.push(block);
        }
      } else {
        projected.push(block);
      }
    }
    *content = projected;
  }
  Ok(images && !unsupported && !rejected)
}
