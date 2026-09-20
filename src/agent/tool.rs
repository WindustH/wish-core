use super::RunControl;
use serde_json::{Value, json};
use std::future::Future;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ToolCall {
  pub call_id: String,
  pub name: String,
  pub arguments: Value,
}

/// An unknown result is distinct from failure: an external side effect may have happened.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum ToolOutcome {
  Success(Value),
  Failed(String),
  Cancelled,
  Unknown(String),
}

impl ToolOutcome {
  pub(crate) fn encode_content(&self) -> Value {
    match self {
      Self::Success(value) => json!({"status": "success", "output": value}),
      Self::Failed(message) => json!({"status": "failed", "message": message}),
      Self::Cancelled => json!({"status": "cancelled"}),
      Self::Unknown(message) => json!({"status": "unknown", "message": message}),
    }
  }
}

/// Executors must observe cancellation and return after their work has stopped or its outcome
/// is known to be unknown. The loop never drops a started execution to pretend it was cancelled.
/// Futures must not panic. Parallel mode requires tools safe to execute concurrently.
pub trait ToolExecutor: Sync {
  fn execute(
    &self,
    call: &ToolCall,
    control: &RunControl,
  ) -> impl Future<Output = ToolOutcome> + Send;
}
