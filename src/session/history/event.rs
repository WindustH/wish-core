use crate::protocol::{
  StreamEvent,
  account_state::AccountState,
  model_use::{
    response::{StopReason, Usage},
    stream::PartialResponse,
  },
};
use crate::session::{
  EntryId, GenerationId, RunOutcome, SessionConfig, SessionPhase, TokenMeasurement, ToolCall,
  ToolOutcome,
};
use crate::storage::ListId;
use serde_json::Value;

/// Owned event payloads. HistoryRecord carries the timestamp and originating model call.
/// ModelStream is delivered live only by the executor; legacy stored variants remain readable.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum SessionEvent {
  UpstreamCompactionStarted {
    generation: GenerationId,
  },
  UpstreamCompactionCompleted(Box<crate::protocol::upstream_compaction::UpstreamCompaction>),
  CompactionSummaryStarted {
    source_start: u64,
    source_end: u64,
    measurement: TokenMeasurement,
  },
  CompactionSummary {
    generation: GenerationId,
    source_start: u64,
    source_end: u64,
    entry: EntryId,
    response: Box<crate::protocol::Response>,
  },
  /// Preparing a standby segment is opportunistic: its failure is recorded for inspection but
  /// never turns an otherwise successful conversation run into a suspended one.
  CompactionSummaryFailed {
    outcome: RunOutcome,
  },
  CompactionTranslationStarted {
    generation: GenerationId,
  },
  CompactionTranslationFailed {
    outcome: RunOutcome,
  },
  CompactionTranslationCompleted {
    previous: GenerationId,
    active: GenerationId,
    entry: EntryId,
    translated: bool,
  },
  ContextCompacted {
    previous: GenerationId,
    active: GenerationId,
    reason: crate::session::CompactionReason,
    removed_entries: u64,
    measurement: TokenMeasurement,
  },
  Created(Box<SessionConfig>),
  /// No longer recorded; kept so the history of earlier builds still loads.
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
  /// A note of the application's, recorded for its own account - a message it delivered elsewhere,
  /// say - which the engine neither reads nor acts on.
  Application(Value),
}
impl SessionEvent {
  /// Whether the event belongs to the model call running as it is recorded. Settings, input and
  /// context switches happen beside any call.
  pub(in crate::session) fn belongs_to_call(&self) -> bool {
    !matches!(
      self,
      Self::Created(_)
        | Self::ContextEntryCreated { .. }
        | Self::MetadataUpdated(_)
        | Self::ConfigUpdated(_)
        | Self::InputMoved { .. }
        | Self::MessageQueued { .. }
        | Self::InputsConsumed { .. }
        | Self::GenerationPrepared { .. }
        | Self::GenerationActivated { .. }
        | Self::Application(_)
    )
  }
  /// The variant's name, which history filters select events by.
  pub(in crate::session) fn type_name(&self) -> &'static str {
    match self {
      Self::UpstreamCompactionStarted { .. } => "UpstreamCompactionStarted",
      Self::UpstreamCompactionCompleted(..) => "UpstreamCompactionCompleted",
      Self::CompactionSummaryStarted { .. } => "CompactionSummaryStarted",
      Self::CompactionSummary { .. } => "CompactionSummary",
      Self::CompactionSummaryFailed { .. } => "CompactionSummaryFailed",
      Self::CompactionTranslationStarted { .. } => "CompactionTranslationStarted",
      Self::CompactionTranslationFailed { .. } => "CompactionTranslationFailed",
      Self::CompactionTranslationCompleted { .. } => "CompactionTranslationCompleted",
      Self::ContextCompacted { .. } => "ContextCompacted",
      Self::Created(..) => "Created",
      Self::ContextEntryCreated { .. } => "ContextEntryCreated",
      Self::ResponseRejected(..) => "ResponseRejected",
      Self::MetadataUpdated(..) => "MetadataUpdated",
      Self::ConfigUpdated(..) => "ConfigUpdated",
      Self::MessageQueued { .. } => "MessageQueued",
      Self::InputMoved { .. } => "InputMoved",
      Self::InputCancelled { .. } => "InputCancelled",
      Self::InputsConsumed { .. } => "InputsConsumed",
      Self::StateChanged { .. } => "StateChanged",
      Self::TurnStarted { .. } => "TurnStarted",
      Self::ModelStream(..) => "ModelStream",
      Self::ResponseAccepted { .. } => "ResponseAccepted",
      Self::ResponseInterrupted(..) => "ResponseInterrupted",
      Self::ToolStarted(..) => "ToolStarted",
      Self::ToolFinished { .. } => "ToolFinished",
      Self::StableBoundary { .. } => "StableBoundary",
      Self::Finished(..) => "Finished",
      Self::GenerationPrepared { .. } => "GenerationPrepared",
      Self::GenerationActivated { .. } => "GenerationActivated",
      Self::Application(..) => "Application",
    }
  }
}
