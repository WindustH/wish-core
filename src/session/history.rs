use super::GenerationId;
use crate::protocol::Message;
use crate::session::statistics::{ModelCallId, Timestamp};

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
  /// Time this entry was created, before dispatching its storage transaction. Legacy rows lack it.
  #[serde(default)]
  pub recorded_at: Option<Timestamp>,
  /// Originating model call, shared by all its response/replay messages.
  #[serde(default)]
  pub model_call_id: Option<ModelCallId>,
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
  /// Event creation/receipt time, retained across batched writes. None for legacy history.
  #[serde(default)]
  pub recorded_at: Option<Timestamp>,
  #[serde(default)]
  pub model_call_id: Option<ModelCallId>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventId(pub u64);
