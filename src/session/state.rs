use super::{EventId, RunOutcome};
use crate::executor::tool::{ToolCall, ToolOutcome};
use crate::protocol::Request;
use crate::storage::ListId;
use std::sync::Arc;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
  Idle,
  Ready,
  CallingModel,
  ExecutingTools,
  Suspended,
}

/// Progress belongs to the session; live network futures belong to the runner.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum SessionState {
  Idle,
  Ready { completed_turns: usize, needs_model: bool },
  CallingModel { turn: usize },
  ExecutingTools { turn: usize, batch: ListId },
  Suspended { outcome: EventId },
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ToolExecution {
  pub call: ToolCall,
  pub started: bool,
  pub outcome: Option<ToolOutcome>,
}

impl SessionState {
  pub fn get_phase(&self) -> SessionPhase {
    match self {
      Self::Idle => SessionPhase::Idle,
      Self::Ready { .. } => SessionPhase::Ready,
      Self::CallingModel { .. } => SessionPhase::CallingModel,
      Self::ExecutingTools { .. } => SessionPhase::ExecutingTools,
      Self::Suspended { .. } => SessionPhase::Suspended,
    }
  }
  pub fn is_stable(&self) -> bool {
    matches!(self, Self::Idle | Self::Ready { .. } | Self::Suspended { .. })
  }
}

/// Only the runner executes these effects. Selecting one updates SessionState first.
pub(crate) enum SessionAction {
  CallModel(Arc<Request>),
  ExecuteTools(Vec<ToolCall>),
  Finished(RunOutcome),
}
