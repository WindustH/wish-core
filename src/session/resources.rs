//! References to stored resources carried by session messages.
use crate::{
  protocol::Message,
  storage::blobs::{blob_id, is_blob_id},
};
use base64::Engine;
use serde_json::Value;
use std::collections::HashSet;
/// The session files a message names: blobs by the SHA-256 of their bytes, which is also what an
/// inline image hashes to, and shell executions by their id. Any word shaped like one counts;
/// only those on disk matter.
pub fn collect_references(message: &Message, found: &mut HashSet<String>) {
  if let Ok(value) = serde_json::to_value(message) {
    collect(&value, found);
  }
}
pub(crate) fn collect(value: &Value, found: &mut HashSet<String>) {
  match value {
    Value::String(text) => {
      for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        if is_blob_id(word) || is_execution_id(word) {
          found.insert(word.to_owned());
        } else if uuid::Uuid::parse_str(word).is_ok() {
          found.insert(format!("snapshot:{word}"));
        }
      }
    }
    Value::Array(items) => items.iter().for_each(|item| collect(item, found)),
    Value::Object(fields) => {
      for (key, item) in fields {
        match (key.as_str(), item) {
          ("data_base64", Value::String(data)) => {
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
              found.insert(blob_id(&bytes));
            }
          }
          _ => collect(item, found),
        }
      }
    }
    _ => {}
  }
}
pub use crate::utils::resource_id::is_execution_id;

impl super::SessionSender {
  /// Run a resource operation against the committed active/standby generations and pending
  /// inputs, serialized with context/queue publication. Only ImageRef needs bytes from disk;
  /// text mentions and inline images do not create that dependency.
  pub fn with_image_dependencies<R: Send + 'static>(
    &self,
    apply: impl FnOnce(HashSet<String>) -> R + Send + 'static,
  ) -> Result<R, super::SessionError> {
    self.transact(move |transaction| {
      let mut images = HashSet::new();
      for id in transaction.used_entries()? {
        let entry = transaction
          .store
          .get_item::<super::Entry>(&transaction.record.entries, id as u64)?
          .ok_or(super::SessionError::InvalidEntry(super::EntryId(id)))?;
        let content = match &entry.message {
          Message::User { content, .. }
          | Message::Assistant { content, .. }
          | Message::System { content, .. }
          | Message::Developer { content, .. } => content,
          _ => continue,
        };
        for block in content {
          if let crate::protocol::ContentBlock::ImageRef { blob_id, .. } = block {
            images.insert(blob_id.clone());
          }
        }
      }
      Ok(apply(images))
    })
  }
}
