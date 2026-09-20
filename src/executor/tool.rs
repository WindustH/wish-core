use super::ExecutionControl;
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
    control: &ExecutionControl,
  ) -> impl Future<Output = ToolOutcome> + Send;
}

use super::observe::notify_observers;
use crate::session::{Session, SessionError, SessionEvent, ToolMode};
use futures_util::future::join_all;

pub(super) async fn execute_tools(
  executor: &impl ToolExecutor,
  session: &mut Session,
  calls: &[ToolCall],
  control: &ExecutionControl,
  cursor: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<(), SessionError> {
  match session.get_config().run.tools {
    ToolMode::Serial => {
      let mut unknown = false;
      for call in calls {
        let outcome = if unknown || control.is_cancelled() {
          ToolOutcome::Cancelled
        } else if !is_registered(session, call) {
          ToolOutcome::Failed(format!("unknown tool: {}", call.name))
        } else {
          session.start_tool(call)?;
          notify_observers(session, cursor, observe)?;
          // A ToolStarted observer can request cancellation before any external effect.
          if control.is_cancelled() {
            ToolOutcome::Cancelled
          } else {
            executor.execute(call, control).await
          }
        };
        unknown |= matches!(outcome, ToolOutcome::Unknown(_));
        session.accept_tool_outcome(call, outcome)?;
        notify_observers(session, cursor, observe)?;
      }
    }
    ToolMode::Parallel => {
      let mut futures = Vec::new();
      for call in calls {
        let registered = is_registered(session, call);
        if registered && !control.is_cancelled() {
          session.start_tool(call)?;
          notify_observers(session, cursor, observe)?;
        }
        futures.push(async move {
          if control.is_cancelled() {
            ToolOutcome::Cancelled
          } else if !registered {
            ToolOutcome::Failed(format!("unknown tool: {}", call.name))
          } else {
            executor.execute(call, control).await
          }
        });
      }
      for (call, outcome) in calls.iter().zip(join_all(futures).await) {
        session.accept_tool_outcome(call, outcome)?;
        notify_observers(session, cursor, observe)?;
      }
    }
  }
  Ok(())
}
fn is_registered(session: &Session, call: &ToolCall) -> bool {
  session.get_config().tools.iter().any(|tool| tool.name == call.name)
}
