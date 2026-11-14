//! Projects stored images for a model call without changing the original context.
use crate::{
  protocol::error::Error,
  protocol::{ContentBlock, Message},
  storage::blobs,
};
use base64::Engine;
use serde_json::Value;
use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
};
/// Materializes retained images within the model's capabilities and byte budget.
/// Returns admission decisions, never prompt text. With `save`, legacy inline images
/// are retained as blobs; ordinary inputs were already saved at admission.
pub async fn project_images(
  models: &BTreeMap<String, Value>,
  model: &str,
  blob_dir: &Path,
  rejected: bool,
  messages: &mut [Message],
  save: bool,
  materialize: bool,
) -> Result<(bool, Vec<ImageDelivery>), Error> {
  let unsupported = models
    .get(model)
    .and_then(|value| value["input_modalities"].as_array())
    .is_some_and(|values| !values.iter().any(|value| value == "image"));
  let mut images = false;
  let mut deliveries = Vec::new();
  let mut remaining = crate::utils::model_image::MAX_WIRE_BYTES;
  let source_paths: BTreeMap<String, String> = messages
    .iter()
    .filter_map(|message| {
      if let Message::ToolResult { name, call_id, content, .. } = message
        && name == "view_image"
        && let Some(path) = content["output"]["path"].as_str()
      {
        Some((call_id.clone(), path.to_owned()))
      } else {
        None
      }
    })
    .collect();
  // Prefer the newest images; older ones remain accessible through their file references.
  for message in messages.iter_mut().rev() {
    let (from_view_image, source_path) = match message {
      Message::User { metadata, .. } if metadata["tool_name"] == "view_image" => {
        let call = metadata["tool_call_id"].as_str().or_else(|| metadata["call_id"].as_str());
        (
          true,
          call
            .and_then(|call| source_paths.get(call).cloned())
            .or_else(|| metadata["result"]["output"]["path"].as_str().map(str::to_owned)),
        )
      }
      _ => (false, None),
    };
    let content = match message {
      Message::User { content, .. }
      | Message::Assistant { content, .. }
      | Message::System { content, .. }
      | Message::Developer { content, .. } => content,
      _ => continue,
    };
    let mut projected = Vec::new();
    for block in std::mem::take(content).into_iter().rev() {
      let omitted = if unsupported {
        Some("model does not support images")
      } else if rejected {
        Some("image input rejected by upstream")
      } else {
        None
      };
      let (path, image) = match block {
        ContentBlock::ImageRef { blob_id, byte_count, mime_type } => {
          if !blobs::is_blob_id(&blob_id)
            || byte_count > crate::utils::model_image::MAX_OUTPUT_BYTES
          {
            return Err(Error::Build("invalid stored image reference".into()));
          }
          let path = blob_dir.join(&blob_id);
          let wire_bytes = byte_count.div_ceil(3) * 4;
          let image = if omitted.is_some() || wire_bytes > remaining {
            None
          } else {
            remaining -= wire_bytes;
            if materialize {
              let bytes = crate::utils::bounded_file::read(&path, byte_count)
                .await
                .map_err(|error| Error::Build(format!("read stored image: {error}")))?;
              Some(
                crate::utils::blocking::cpu(move || {
                  if bytes.len() != byte_count || blobs::blob_id(&bytes) != blob_id {
                    return Err(Error::Build("stored image integrity mismatch".into()));
                  }
                  let mime = crate::utils::image_format::supported_mime(&bytes)
                    .ok_or_else(|| Error::Build("invalid stored image format".into()))?;
                  Ok(ContentBlock::Image {
                    mime_type: mime.into(),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
                  })
                })
                .await
                .map_err(|e| Error::Build(e.to_string()))??,
              )
            } else {
              Some(ContentBlock::ImageRef { blob_id, byte_count, mime_type })
            }
          };
          (path, image)
        }
        ContentBlock::Image { data_base64, .. } => {
          if data_base64.len() > crate::utils::model_image::MAX_SOURCE_BYTES.div_ceil(3) * 4 {
            return Err(Error::Build("stored image exceeds 100 MiB".into()));
          }
          let (bytes, id, image, length) = crate::utils::model_image::process(move || {
            let bytes = base64::engine::general_purpose::STANDARD
              .decode(data_base64)
              .map_err(|e| e.to_string())?;
            let id = blobs::blob_id(&bytes);
            let mut image = None;
            let mut length = 0;
            if omitted.is_none() && remaining > 0 {
              let prepared = crate::utils::model_image::prepare(&bytes)?;
              let wire_bytes = prepared.bytes.len().div_ceil(3) * 4;
              if wire_bytes <= remaining {
                length = wire_bytes;
                image = Some(if materialize {
                  ContentBlock::Image {
                    mime_type: prepared.mime.into(),
                    data_base64: base64::engine::general_purpose::STANDARD.encode(&prepared.bytes),
                  }
                } else {
                  ContentBlock::ImageRef {
                    mime_type: prepared.mime.into(),
                    blob_id: id.clone(),
                    byte_count: bytes.len(),
                  }
                });
              }
            }
            Ok((bytes, id, image, length))
          })
          .await
          .map_err(Error::Build)?;
          remaining -= length;
          let path = blob_dir.join(&id);
          if save {
            let directory = blob_dir.to_path_buf();
            crate::utils::blocking::io(move || blobs::save_image_bytes(&directory, &bytes))
              .await
              .map_err(|e| Error::Build(e.to_string()))?
              .map_err(|e| Error::Build(e.to_string()))?;
          }
          (path, image)
        }
        other => {
          projected.push(other);
          continue;
        }
      };
      let reason = if unsupported {
        Some("model does not support images")
      } else if rejected {
        Some("image input rejected by upstream")
      } else if image.is_none() {
        Some("image budget exceeded")
      } else {
        None
      };
      deliveries.push(ImageDelivery {
        path,
        reason,
        from_view_image,
        source_path: source_path.clone(),
      });
      // Blocks are assembled backwards, then restored to their original order below.
      if let Some(image) = image {
        images = true;
        projected.push(image);
      }
    }
    projected.reverse();
    *content = projected;
  }
  Ok((images, deliveries))
}

/// An admission decision, not generated prompt text. Its durable event owns any explanation.
pub struct ImageDelivery {
  pub path: PathBuf,
  pub reason: Option<&'static str>,
  pub from_view_image: bool,
  pub source_path: Option<String>,
}

/// Save image input and its path notice together, before it enters session history.
/// Repeated admission (input endpoint -> session queue) is idempotent.
pub async fn retain_input(blob_dir: &Path, message: &mut Message) -> Result<(), Error> {
  if matches!(message, Message::User { metadata, .. } if metadata["tool_name"] == "view_image") {
    return Ok(());
  }
  let content = match message {
    Message::User { content, .. }
    | Message::Assistant { content, .. }
    | Message::System { content, .. }
    | Message::Developer { content, .. } => content,
    _ => return Ok(()),
  };
  let mut retained = Vec::new();
  for block in std::mem::take(content) {
    let block = if let ContentBlock::Image { data_base64, .. } = block {
      if data_base64.len() > crate::utils::model_image::MAX_SOURCE_BYTES.div_ceil(3) * 4 {
        return Err(Error::Build("image exceeds 100 MiB".into()));
      }
      let prepared = crate::utils::model_image::process(move || {
        let bytes = base64::engine::general_purpose::STANDARD
          .decode(data_base64)
          .map_err(|e| e.to_string())?;
        crate::utils::model_image::prepare(&bytes)
      })
      .await
      .map_err(Error::Build)?;
      let directory = blob_dir.to_path_buf();
      crate::utils::blocking::io(move || {
        blobs::save_image_bytes(&directory, &prepared.bytes)
          .map_err(|e| Error::Build(e.to_string()))?;
        Ok::<_, Error>(ContentBlock::ImageRef {
          mime_type: prepared.mime.into(),
          blob_id: blobs::blob_id(&prepared.bytes),
          byte_count: prepared.bytes.len(),
        })
      })
      .await
      .map_err(|e| Error::Build(e.to_string()))??
    } else {
      block
    };
    if let ContentBlock::ImageRef { blob_id, .. } = &block {
      if !blobs::is_blob_id(blob_id) {
        return Err(Error::Build("invalid stored image reference".into()));
      }
      let directory = blob_dir.to_path_buf();
      let id = blob_id.clone();
      let alias =
        super::error::blocking(move || super::env_aliases::Aliases::open(&directory)?.blob(&id))
          .await
          .map_err(|e| Error::Build(e.to_string()))?;
      let text = format!("Image read and stored at \"$WISH_BLOB_ROOT/{alias}\".");
      if !matches!(retained.last(), Some(ContentBlock::Text { text: previous }) if previous == &text)
      {
        retained.push(ContentBlock::Text { text });
      }
    }
    retained.push(block);
  }
  *content = retained;
  Ok(())
}
