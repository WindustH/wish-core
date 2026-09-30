//! Persistent sessions: state transitions, context, history and transactional storage.
mod config;
mod context;
mod error;
pub mod history;
mod lifecycle;
mod machine;
mod persistence;
mod queue;
mod reader;
pub mod statistics;
mod tool;

#[allow(unused_imports)] // wish-test
pub use config::RunOptions;
pub use config::{SessionConfig, ToolMode};
pub use context::{
  CompactionConfig, CompactionReason, Generation, GenerationId, GenerationStatus, TokenEstimator,
  TokenMeasurement, TokenMeasurementSource,
};
pub use error::SessionError;
pub use history::{Entry, EntryId, EntryOrigin, EventId, HistoryItem, HistoryRecord, SessionEvent};
pub(crate) use machine::SessionAction;
pub use machine::{RunOutcome, SessionPhase, SessionState};
pub use queue::SessionSender;
pub use reader::SessionReader;
pub use tool::{ToolCall, ToolOutcome};

use crate::storage::{OwnerGuard, Storage};
use persistence::SessionRecord;

/// A single running owner backed by transactional storage. Other tasks use a `SessionSender` to
/// queue input and a `SessionReader` to read.
pub struct Session {
  storage: Storage,
  key: String,
  _owner: OwnerGuard,
  arrivals: queue::InputArrivals,
  reader: SessionReader,
  record: SessionRecord,
}
