//! Application data on an encrypted compaction item: the provider that can read it, and the
//! readable handoff every other provider reads in its place.
//!
//! Both ride in the item's metadata, which never reaches a model. The item itself stays in the
//! context unchanged, so switching back to the provider that made it resumes from the encrypted
//! history instead of from a retelling of it.
use crate::protocol::{ContentBlock, Message};
use serde_json::{Value, json};

/// Whether `provider` can read the item's encrypted content: only the provider that made it can.
pub(crate) fn can_read(item: &Message, provider: &str) -> bool {
  item.get_metadata().get("provider").and_then(Value::as_str) == Some(provider)
}

/// Notes the provider that made the item.
pub(crate) fn set_provider(item: &mut Message, provider: &str) {
  let mut metadata = item.get_metadata().clone();
  if !metadata.is_object() {
    metadata = json!({});
  }
  metadata["provider"] = json!(provider);
  item.set_metadata(metadata);
}

/// The readable handoff, and whether it retells the item (false: a placeholder that says the
/// history could not be retold, to be translated again when a provider that can read it is back).
pub(crate) fn get_handoff(item: &Message) -> Option<(Vec<ContentBlock>, bool)> {
  let handoff = item.get_metadata().get("handoff")?;
  let content = serde_json::from_value(handoff.get("content")?.clone()).ok()?;
  Some((content, handoff.get("translated").and_then(Value::as_bool).unwrap_or(false)))
}

/// A copy of the item carrying this handoff.
pub(crate) fn with_handoff(item: &Message, content: &[ContentBlock], translated: bool) -> Message {
  let mut item = item.clone();
  let mut metadata = item.get_metadata().clone();
  if !metadata.is_object() {
    metadata = json!({});
  }
  metadata["handoff"] = json!({"content": content, "translated": translated});
  item.set_metadata(metadata);
  item
}

/// The conversation as `provider` receives it: each item it cannot read and that has a handoff
/// becomes that handoff, as an instruction in the item's place. An item without one stays, and
/// the wire refuses it rather than sending history it cannot read.
pub(crate) fn project(conversation: &mut [Message], provider: &str) {
  for message in conversation {
    if !matches!(message, Message::UpstreamCompaction { .. }) || can_read(message, provider) {
      continue;
    }
    if let Some((content, _)) = get_handoff(message) {
      *message = Message::Developer {
        metadata: json!({"source": "upstream_compaction_handoff"}),
        fixed: Some(false),
        content,
      };
    }
  }
}
