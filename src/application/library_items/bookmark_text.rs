//! Bookmark coordinates address displayed message blocks, including the user's original draft.
//! Keep this projection aligned with wish-web's orderedUserContent/userBubbles; wire content can
//! contain expanded reply descriptions and adjacent text blocks that the reader never sees.
use crate::protocol::{ContentBlock, Message};
use serde_json::Value;
use std::collections::HashSet;

fn native(content: &[ContentBlock]) -> Vec<Option<String>> {
  content
    .iter()
    .map(|block| match block {
      ContentBlock::Text { text } => Some(text.clone()),
      _ => None,
    })
    .collect()
}
fn reference(value: &Value) -> bool {
  let integer = |key: &str| value[key].as_u64().filter(|n| *n <= 9_007_199_254_740_991);
  ["id", "session", "source", "label", "text"].iter().all(|key| value[key].is_string())
    && value["text"].as_str().is_some_and(|text| text.encode_utf16().count() <= 16000)
    && (value.get("seq").is_none() || integer("seq").is_some())
    && (value.get("start").is_none()
      || integer("start").zip(integer("end")).is_some_and(|(a, b)| a <= b))
    && (value.get("lineStart").is_none()
      || integer("lineStart").zip(integer("lineEnd")).is_some_and(|(a, b)| a > 0 && a <= b))
}
fn user(metadata: &Value, content: &[ContentBlock]) -> Vec<Option<String>> {
  let attachments = metadata["attachments"].as_array();
  let custom = &metadata["custom"];
  let parts = if let (Some(draft), Some(references)) =
    (custom["reply_draft"].as_str(), custom["reply_references"].as_array())
  {
    let tokens: HashSet<String> = references
      .iter()
      .filter(|v| reference(v))
      .filter_map(|v| v["id"].as_str().map(|id| format!("<reply-{id}>")))
      .chain(
        attachments
          .into_iter()
          .flatten()
          .filter_map(|v| v["placeholder"].as_str().map(str::to_owned)),
      )
      .collect();
    let mut parts = Vec::new();
    let mut offset = 0;
    for (index, _) in draft.match_indices('<') {
      let Some(end) = draft.as_bytes()[index..]
        .iter()
        .take(72)
        .position(|byte| *byte == b'>')
        .map(|end| index + end + 1)
      else {
        continue;
      };
      let token = &draft[index..end];
      let recognized = ["<image-", "<file-", "<paste-", "<reply-"].iter().any(|prefix| {
        token
          .strip_prefix(prefix)
          .and_then(|rest| rest.strip_suffix('>'))
          .is_some_and(crate::storage::blobs::is_blob_id)
      });
      if !recognized || !tokens.contains(token) {
        continue;
      }
      if offset < index {
        parts.push(Some(draft[offset..index].into()));
      }
      parts.push(None);
      offset = end;
    }
    if offset < draft.len() {
      parts.push(Some(draft[offset..].into()));
    }
    parts
  } else if let Some(parts) = metadata["input_parts"].as_array() {
    parts
      .iter()
      .filter_map(|part| {
        if part["type"] == "text" {
          part["text"].as_str().map(|s| Some(s.to_owned()))
        } else {
          part["index"]
            .as_u64()
            .and_then(|index| attachments.and_then(|a| a.get(index as usize)))
            .map(|_| None)
        }
      })
      .collect()
  } else {
    native(content)
  };
  let mut bubbles: Vec<Option<String>> = Vec::new();
  for part in parts {
    if let Some(text) = part {
      if text.is_empty() {
        continue;
      }
      if let Some(Some(previous)) = bubbles.last_mut() {
        previous.push_str(&text);
      } else {
        bubbles.push(Some(text));
      }
    } else {
      bubbles.push(None);
    }
  }
  bubbles.retain(|part| part.as_ref().is_none_or(|text| !text.trim().is_empty()));
  bubbles
}
pub(super) fn blocks(message: &Message) -> Option<Vec<Option<String>>> {
  match message {
    Message::User { content, metadata } => Some(user(metadata, content)),
    Message::Assistant { content, .. } => Some(native(content)),
    _ => None,
  }
}
