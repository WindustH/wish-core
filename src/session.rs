//! Persistent sessions: state transitions, context, history and transactional storage.
mod config;
mod context;
mod control;
mod error;
pub mod history;
mod lifecycle;
mod machine;
mod persistence;
mod queue;
pub mod statistics;

pub use config::{RunOptions, SessionConfig, ToolMode};
pub use context::{CompactionConfig, CompactionReason, Generation, GenerationId, GenerationStatus};
pub use control::SessionHandle;
pub use error::SessionError;
pub use history::HistoryReader;
pub use history::{Entry, EntryId, EntryOrigin, EventId, HistoryItem, HistoryRecord, SessionEvent};
pub(crate) use machine::SessionAction;
pub use machine::{RunOutcome, SessionPhase, SessionState, ToolExecution};
pub use queue::SessionSender;

use crate::storage::{OwnerGuard, Storage};
use persistence::SessionRecord;

/// A single running owner backed by transactional storage. Other tasks use SessionSender.
pub struct Session {
  storage: Storage,
  key: String,
  _owner: OwnerGuard,
  control: control::SessionControl,
  record: SessionRecord,
}
