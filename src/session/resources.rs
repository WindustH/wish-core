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
fn collect(value: &Value, found: &mut HashSet<String>) {
  match value {
    Value::String(text) => {
      for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        if is_blob_id(word) || is_execution_id(word) {
          found.insert(word.to_owned());
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
