//! Execute and settle one batch of tool calls.
use super::{ExecutionControl, observe::deliver_new_events};
use crate::session::{Session, SessionError, SessionEvent, ToolCall, ToolMode, ToolOutcome};
pub use crate::tool::contract::{ToolExecutor, ToolInput};
use futures_util::{StreamExt, stream::FuturesUnordered};

pub(super) async fn execute_tools(
  executor: &impl ToolExecutor,
  session: &mut Session,
  calls: &[ToolCall],
  control: &ExecutionControl,
  delivered: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<(), SessionError> {
  let input = session.get_config().run.background_tools_on_input.then(|| ToolInput::new(session));
  match session.get_config().run.tools {
    ToolMode::Serial => {
      let mut unknown = false;
      let mut yielded = false;
      for call in calls {
        let outcome = if unknown || control.is_cancelled() {
          ToolOutcome::Cancelled
        } else if yielded {
          ToolOutcome::Report {
            text: "Not executed: earlier work continues in background and new input is waiting. Reconsider this call after reading the new input.".into(),
            output: serde_json::json!({"status":"deferred","executed":false}),
            metadata: serde_json::Value::Null, failed: false,
          }
        } else if !is_registered(session, call) && !executor.accepts_unadvertised_call(&call.name) {
          ToolOutcome::Failed(format!("unknown tool: {}", call.name))
        } else {
          session.start_tool(call)?;
          deliver_new_events(session, delivered, observe)?;
          // A ToolStarted observer can request cancellation before any external effect.
          if control.is_cancelled() {
            ToolOutcome::Cancelled
          } else {
            executor.execute_with_input(call, control, input.clone()).await
          }
        };
        yielded |= match &outcome {
          ToolOutcome::Report { output, .. } => output["yielded_for_input"] == true,
          _ => false,
        };
        unknown |= matches!(outcome, ToolOutcome::Unknown(_));
        session.accept_tool_outcome(call, outcome)?;
        deliver_new_events(session, delivered, observe)?;
      }
    }
    ToolMode::Parallel => {
      let mut futures = FuturesUnordered::new();
      for call in calls {
        let registered =
          is_registered(session, call) || executor.accepts_unadvertised_call(&call.name);
        if registered && !control.is_cancelled() {
          session.start_tool(call)?;
          deliver_new_events(session, delivered, observe)?;
        }
        let input = input.clone();
        futures.push(async move {
          let outcome = if control.is_cancelled() {
            ToolOutcome::Cancelled
          } else if !registered {
            ToolOutcome::Failed(format!("unknown tool: {}", call.name))
          } else {
            executor.execute_with_input(call, control, input).await
          };
          (call, outcome)
        });
      }
      while let Some((call, outcome)) = futures.next().await {
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
