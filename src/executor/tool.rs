//! The contract a tool implementation fulfils, and the execution of one tool batch in the session's
//! tool mode: calls are started, carried out and settled in the session, each step delivered.
use super::{ExecutionControl, observe::deliver_new_events};
use crate::session::{Session, SessionError, SessionEvent, ToolCall, ToolMode, ToolOutcome};
use futures_util::{stream::FuturesUnordered, StreamExt};
use crate::session::SessionReader;
use tokio::sync::watch;
use std::future::Future;

/// An event subscription installed before checking the durable queue. No timer or polling.
/// Background completions wake idle sessions, but do not preempt unrelated foreground work.
#[derive(Clone)]
pub struct ToolInput {
  reader: SessionReader,
  head: u64,
  arrivals: watch::Receiver<()>,
}
impl ToolInput {
  fn new(session: &Session) -> Self {
    let arrivals = session.watch_input_arrivals();
    Self { reader: session.reader().clone(), head: session.get_queue_head(), arrivals }
  }
  pub fn pending(&self) -> Result<bool, SessionError> {
    for page in self.reader.get_message_queue().pages(self.head) {
      for id in page?.items {
        if let Some(entry) = self.reader.get_entry(*id)? {
          let source = entry.message.get_metadata()["source"].as_str().unwrap_or_default();
          if !matches!(source, "background_execution_finished" | "background_tool_finished") {
            return Ok(true);
          }
        }
      }
    }
    Ok(false)
  }
  pub async fn wait(&mut self) -> Result<(), SessionError> {
    loop {
      if self.pending()? { return Ok(()); }
      // A closed session will never receive new input. It is cancelled by its owner instead.
      if self.arrivals.changed().await.is_err() { std::future::pending::<()>().await; }
    }
  }
}

/// Executors must observe cancellation and return after their work has stopped or its outcome
/// is known to be unknown. The loop never drops a started execution to pretend it was cancelled.
/// Futures must not panic. Parallel mode requires tools safe to execute concurrently.
pub trait ToolExecutor: Sync {
  /// Applications may accept enabled capabilities through an alternative, unadvertised
  /// route. Generic executors retain strict registration unless they opt in explicitly.
  fn accepts_unadvertised_call(&self, _name: &str) -> bool {
    false
  }

  /// Opt-in application hook. Generic engine fixtures retain the ordinary execution contract.
  fn execute_with_input(
    &self, call: &ToolCall, control: &ExecutionControl, _input: Option<ToolInput>,
  ) -> impl Future<Output = ToolOutcome> + Send {
    self.execute(call, control)
  }

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
