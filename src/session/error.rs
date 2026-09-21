use super::EntryId;
use crate::storage::StorageError;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
  #[error("invalid history query: {0}")]
  InvalidHistoryQuery(String),
  #[error(transparent)]
  Storage(#[from] StorageError),
  #[error("operation requires a stable session boundary")]
  Busy,
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
