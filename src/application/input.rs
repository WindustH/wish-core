//! Ordered user content. Editor tokens refer to session-local uploaded blobs;
//! original text and the display order remain in metadata for queue editing.
use crate::{
  application::{
    blobs::{self, is_blob_id},
    error::ApplicationError,
  },
  protocol::{ContentBlock, Message},
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
  pub id: String,
  pub kind: String,
  pub name: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  byte_count: Option<usize>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub placeholder: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
  #[serde(default)]
  pub background_shell: bool,
  pub text: String,
  #[serde(default)]
  pub attachments: Vec<Attachment>,
  #[serde(default)]
  pub metadata: Value,
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Part {
  Text { text: String },
  Attachment { index: usize },
}
fn ordered_parts(input: &Input) -> Result<Vec<Part>, ApplicationError> {
  let mut tokens = HashSet::new();
  let mut occurrences = Vec::new();
  for (index, attachment) in input.attachments.iter().enumerate() {
    if let Some(token) = &attachment.placeholder {
      let prefix = format!("<{}-", attachment.kind);
      let digest = token.strip_prefix(&prefix).and_then(|s| s.strip_suffix('>'));
      if !matches!(attachment.kind.as_str(), "image" | "file")
        || !digest.is_some_and(|s| is_blob_id(s) && s == attachment.id)
        || !tokens.insert(token)
      {
        return Err(ApplicationError::invalid("invalid or duplicate attachment placeholder"));
      }
      for (offset, _) in input.text.match_indices(token) {
        occurrences.push((offset, token.len(), index));
      }
    }
  }
  occurrences.sort_unstable_by_key(|item| item.0);
  let mut parts = Vec::new();
  let mut offset = 0;
  for (start, length, index) in occurrences {
    if start > offset {
      parts.push(Part::Text { text: input.text[offset..start].into() });
    }
    parts.push(Part::Attachment { index });
    offset = start + length;
  }
  if offset < input.text.len() {
    parts.push(Part::Text { text: input.text[offset..].into() });
  }
  // Older clients attach images without editor tokens; preserve their order.
  for (index, attachment) in input.attachments.iter().enumerate() {
    if attachment.placeholder.is_none() {
      parts.push(Part::Attachment { index });
    }
  }
  Ok(parts)
}

/// The message an input makes, its attachments read from the session's blobs.
pub async fn message(blob_dir: &Path, mut input: Input) -> Result<Message, ApplicationError> {
  let parts = ordered_parts(&input)?;
  let mut attachments = Vec::new();
  for attachment in &mut input.attachments {
    if !is_blob_id(&attachment.id) {
      return Err(ApplicationError::invalid("invalid attachment id"));
    }
    let blob = blobs::read_description(blob_dir, &attachment.id).await?;
    attachment.byte_count = Some(blob.byte_count);
    let block = match attachment.kind.as_str() {
      "image" => {
        if !blob.mime_type.starts_with("image/") {
          return Err(ApplicationError::invalid("attachment is not PNG, JPEG, GIF or WebP; convert HEIF/HEIC or other formats before sending (renaming the extension does not convert an image)"));
        }
        let bytes = tokio::fs::read(&blob.path).await.map_err(ApplicationError::internal)?;
        ContentBlock::Image {
          mime_type: blob.mime_type,
          data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
      }
      "file" => ContentBlock::Text {
        text: format!(
          "[File sha256:{}] Attached file {}: {}",
          attachment.id,
          json!(attachment.name),
          json!(blob.path)
        ),
      },
      _ => return Err(ApplicationError::invalid("attachment kind must be image or file")),
    };
    attachments.push(block);
  }
  let content: Vec<_> = parts
    .iter()
    .map(|part| match part {
      Part::Text { text } => ContentBlock::Text { text: text.clone() },
      Part::Attachment { index } => attachments[*index].clone(),
    })
    .collect();
  if content.is_empty() {
    return Err(ApplicationError::invalid("message is empty"));
  }
  Ok(Message::User {
    content,
    metadata: json!({"custom":input.metadata,"attachments":input.attachments,"input_text":input.text,"input_parts":parts}),
  })
}

/// The first input travels with session creation, so rejected uploads cannot leave empty sessions.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InitialInput {
  pub request_id: String,
  pub text: String,
  #[serde(default)]
  pub attachments: Vec<InitialAttachment>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InitialAttachment {
  pub kind: String,
  pub name: Option<String>,
  pub placeholder: Option<String>,
  pub data_base64: Option<String>,
  pub id: Option<String>,
}
impl InitialInput {
  pub async fn prepare(self, directory: &Path) -> Result<Message, ApplicationError> {
    let mut attachments = Vec::new();
    for item in self.attachments {
      let blob = match (item.id, item.data_base64) {
        (Some(id), None) if is_blob_id(&id) => blobs::read_description(directory, &id).await?,
        (None, Some(data)) => {
          let bytes = base64::engine::general_purpose::STANDARD.decode(&data)
            .map_err(|_| ApplicationError::invalid("attachment data is not valid base64"))?;
          blobs::save_upload(directory, &bytes).await?
        }
        _ => return Err(ApplicationError::invalid("attachment requires exactly one valid id or data_base64")),
      };
      attachments.push(Attachment { id: blob.id, kind: item.kind, name: item.name,
        placeholder: item.placeholder, byte_count: Some(blob.byte_count) });
    }
    message(directory, Input { text: self.text, attachments, metadata: Value::Null, background_shell: false }).await
  }
}
