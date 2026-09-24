//! Ordered user content. Editor tokens refer to session-local uploaded blobs;
//! original text and the display order remain in metadata for queue editing.
use super::{Blob, valid_blob};
use crate::{
  protocol::{ContentBlock, Message},
  server::{app::App, error::ApiError},
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
  id: String,
  kind: String,
  name: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  byte_count: Option<usize>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  placeholder: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
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
fn ordered_parts(input: &Input) -> Result<Vec<Part>, ApiError> {
  let mut tokens = HashSet::new();
  let mut occurrences = Vec::new();
  for (index, attachment) in input.attachments.iter().enumerate() {
    if let Some(token) = &attachment.placeholder {
      let prefix = format!("<{}-", attachment.kind);
      let digest = token.strip_prefix(&prefix).and_then(|s| s.strip_suffix('>'));
      if !matches!(attachment.kind.as_str(), "image" | "file")
        || !digest.is_some_and(|s| valid_blob(s) && s == attachment.id)
        || !tokens.insert(token)
      {
        return Err(ApiError::bad_request("invalid or duplicate attachment placeholder"));
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

pub async fn message(app: &App, session_id: &str, mut input: Input) -> Result<Message, ApiError> {
  let parts = ordered_parts(&input)?;
  let dir = app.data_dir.join("blobs").join(session_id);
  let mut attachments = Vec::new();
  for attachment in &mut input.attachments {
    if !valid_blob(&attachment.id) {
      return Err(ApiError::bad_request("invalid attachment id"));
    }
    let blob: Blob = serde_json::from_slice(
      &tokio::fs::read(dir.join(format!("{}.json", attachment.id)))
        .await
        .map_err(|_| ApiError::not_found())?,
    )
    .map_err(ApiError::internal)?;
    attachment.byte_count = Some(blob.byte_count);
    let block = match attachment.kind.as_str() {
      "image" => {
        if !blob.mime_type.starts_with("image/") {
          return Err(ApiError::bad_request("attachment is not a supported image"));
        }
        let bytes = tokio::fs::read(&blob.path).await.map_err(ApiError::internal)?;
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
      _ => return Err(ApiError::bad_request("attachment kind must be image or file")),
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
    return Err(ApiError::bad_request("message is empty"));
  }
  Ok(Message::User {
    content,
    metadata: json!({"custom":input.metadata,"attachments":input.attachments,"input_text":input.text,"input_parts":parts}),
  })
}
