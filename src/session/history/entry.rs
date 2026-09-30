use crate::protocol::Message;
use crate::session::statistics::ModelCallId;
use crate::utils::time::Timestamp;

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
impl EntryOrigin {
  /// Whether an entry of this origin was written by the model call running as it is stored, and
  /// carries that call's ID.
  pub(in crate::session) fn carries_call(self) -> bool {
    matches!(self, Self::Model | Self::Interrupted | Self::Summary)
  }
  /// The origin as it is stored and indexed.
  pub(in crate::session) fn as_str(self) -> &'static str {
    match self {
      Self::Imported => "Imported",
      Self::Input => "Input",
      Self::Model => "Model",
      Self::Tool => "Tool",
      Self::Interrupted => "Interrupted",
      Self::Context => "Context",
      Self::Summary => "Summary",
    }
  }
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
