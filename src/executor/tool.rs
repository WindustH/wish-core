//! The contract a tool implementation fulfils, and the execution of one tool batch in the session's
//! tool mode: calls are started, carried out and settled in the session, each step delivered.
use super::{ExecutionControl, observe::deliver_new_events};
use crate::session::{Session, SessionError, SessionEvent, ToolCall, ToolMode, ToolOutcome};
use futures_util::future::join_all;
use std::future::Future;

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

pub(super) async fn execute_tools(
  executor: &impl ToolExecutor,
  session: &mut Session,
  calls: &[ToolCall],
  control: &ExecutionControl,
  delivered: &mut u64,
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
          deliver_new_events(session, delivered, observe)?;
          // A ToolStarted observer can request cancellation before any external effect.
          if control.is_cancelled() {
            ToolOutcome::Cancelled
          } else {
            executor.execute(call, control).await
          }
        };
        unknown |= matches!(outcome, ToolOutcome::Unknown(_));
        session.accept_tool_outcome(call, outcome)?;
        deliver_new_events(session, delivered, observe)?;
      }
    }
    ToolMode::Parallel => {
      let mut futures = Vec::new();
      for call in calls {
        let registered = is_registered(session, call);
        if registered && !control.is_cancelled() {
          session.start_tool(call)?;
          deliver_new_events(session, delivered, observe)?;
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
        deliver_new_events(session, delivered, observe)?;
      }
    }
  }
  Ok(())
}

fn is_registered(session: &Session, call: &ToolCall) -> bool {
  session.get_config().tools.iter().any(|tool| tool.name == call.name)
}
