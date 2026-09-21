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
  Summary,
}

/// Immutable content. Context-only entries need not appear in the conversation transcript.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Entry {
  pub id: EntryId,
  pub origin: EntryOrigin,
  pub message: Message,
  /// Time this entry was created, before dispatching its storage transaction.
  pub recorded_at: Timestamp,
  /// Originating model call, shared by all its response/replay messages.
  pub model_call_id: Option<ModelCallId>,
}
