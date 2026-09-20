use super::GenerationId;
use crate::protocol::Message;

/// IDs are local to one session and are never reused.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntryId(pub usize);

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryOrigin {
  Imported,
  Input,
  Model,
  Tool,
  Interrupted,
  Context,
}

/// Immutable content. Context-only entries need not appear in the conversation transcript.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Entry {
  pub id: EntryId,
  pub origin: EntryOrigin,
  pub message: Message,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum HistoryItem {
  Message(EntryId),
  Event(EventId),
}

/// Ordered facts, independent of generation positions. Sequence numbers start at zero.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct HistoryRecord {
  pub sequence: u64,
  pub generation: GenerationId,
  pub item: HistoryItem,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventId(pub u64);
