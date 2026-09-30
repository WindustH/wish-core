use super::EntryId;
use crate::storage::StorageError;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
  #[error("invalid history query: {0}")]
  InvalidHistoryQuery(String),
  #[error(transparent)]
  Storage(#[from] StorageError),
  /// The operation needs a stable phase - idle, ready or suspended - and a run is under way.
  #[error("operation requires a stable session boundary")]
  Busy,
  /// A run step found the session in a phase other than the one it continues. The text is Busy's:
  /// it is what API clients have always been told.
  #[error("operation requires a stable session boundary")]
  UnexpectedPhase,
  #[error("only user, system and developer messages can be enqueued")]
  InvalidInput,
  #[error("invalid entry reference: {0:?}")]
  InvalidEntry(EntryId),
  #[error("queued input cannot be used before consumption: {0:?}")]
  PendingInput(EntryId),
  #[error("context contains an unpaired or mismatched tool call/result")]
  UnpairedTools,
  #[error("standby was not prepared against the active generation")]
  StaleGeneration,
  #[error("session must be explicitly resumed before running")]
  Suspended,
  #[error("invalid compaction configuration: {0}")]
  InvalidCompaction(String),
}
