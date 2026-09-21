use crate::executor::tool::{ToolCall, ToolOutcome};
use crate::protocol::{
  StreamEvent,
  account_state::AccountState,
  model_use::{
    response::{StopReason, Usage},
    stream::PartialResponse,
  },
};
use crate::session::{EntryId, GenerationId, RunOutcome, SessionConfig, SessionPhase};
use crate::storage::ListId;
use serde_json::Value;

/// Owned event payloads. HistoryRecord carries the timestamp and originating model call.
/// Stream events are observed immediately and then persisted in batches.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum SessionEvent {
  UpstreamCompactionStarted {
    generation: GenerationId,
  },
  UpstreamCompactionCompleted(Box<crate::protocol::upstream_compaction::UpstreamCompaction>),
  CompactionSummaryStarted {
    source_start: u64,
    source_end: u64,
    measurement: crate::executor::model::tokens::TokenMeasurement,
  },
  CompactionSummary {
    generation: GenerationId,
    source_start: u64,
    source_end: u64,
    entry: EntryId,
    response: Box<crate::protocol::Response>,
  },
  ContextCompacted {
    previous: GenerationId,
    active: GenerationId,
    reason: crate::session::CompactionReason,
    removed_entries: u64,
    measurement: crate::executor::model::tokens::TokenMeasurement,
  },
  Created(Box<SessionConfig>),
  ContextEntryCreated {
    entry: EntryId,
  },
  ResponseRejected(Box<crate::protocol::Response>),
  MetadataUpdated(Value),
  ConfigUpdated(Box<SessionConfig>),
  InputMoved {
    entry: EntryId,
    before: Option<EntryId>,
  },
  InputCancelled {
    entry: EntryId,
  },
  MessageQueued {
    entry: EntryId,
  },
  InputsConsumed {
    queue_start: u64,
    queue_end: u64,
  },
  StateChanged {
    from: SessionPhase,
    to: SessionPhase,
  },
  TurnStarted {
    turn: usize,
  },
  ModelStream(StreamEvent),
  ResponseAccepted {
    turn: usize,
    entry_start: u64,
    entry_end: u64,
    stop_reason: StopReason,
    usage: Usage,
    account_state: Option<AccountState>,
  },
  /// Preserves display-only/incomplete material as well as the protocol's replay fragment.
  ResponseInterrupted(Box<PartialResponse>),
  ToolStarted(ToolCall),
  ToolFinished {
    call: ToolCall,
    outcome: ToolOutcome,
  },
  StableBoundary {
    turn: usize,
  },
  Finished(RunOutcome),
  GenerationPrepared {
    generation: GenerationId,
    entries: ListId,
    entry_count: u64,
    source_generation: GenerationId,
    source_length: u64,
  },
  GenerationActivated {
    previous: GenerationId,
    active: GenerationId,
  },
}
