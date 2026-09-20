use super::{ModelCaller, ModelStream, RunControl, ToolCall, ToolExecutor, ToolOutcome};
use crate::session::{
  HistoryItem, RunOutcome, Session, SessionError, SessionEvent, SessionState, ToolMode,
  state::SessionAction,
};
use crate::{
  Error,
  client::CallResponse,
  protocol::{
    Request, Response, StreamAccumulator,
    model_use::stream::{PartialResponse, StreamEnd, StreamFinalization, ToolExecutionState},
  },
};
use futures_util::{
  future::{Either, join_all, select},
  pin_mut,
};

/// Drive a session until idle or suspended. Resume a suspended session explicitly before calling.
/// Observers receive events from this invocation. Stream events are delivered immediately and
/// persisted in batches; other events are recorded before delivery. Use history for older records.
/// Signal cancellation and await completion; dropping this future during I/O leaves an active
/// state that rejects another run rather than silently repeating potentially effective work.
pub async fn run(
  model_caller: &impl ModelCaller,
  session: &mut Session,
  executor: &impl ToolExecutor,
  control: &RunControl,
  mut observe: impl FnMut(&SessionEvent) + Send,
) -> Result<RunOutcome, SessionError> {
  session.require_stable()?;
  if matches!(session.get_state(), SessionState::Suspended { .. }) {
    return Err(SessionError::Suspended);
  }
  let mut cursor = session.get_history().len()?;
  loop {
    session.collect_inputs()?;
    notify_observers(session, &mut cursor, &mut observe)?;
    if control.is_cancelled() && matches!(session.get_state(), SessionState::Ready { .. }) {
      session.finish_run(RunOutcome::Interrupted)?;
    }
    let action = session.advance()?;
    notify_observers(session, &mut cursor, &mut observe)?;
    match action {
      SessionAction::Finished(outcome) => return Ok(outcome),
      SessionAction::CallModel(request) => {
        let result =
          call_model(model_caller, &request, session, control, &mut cursor, &mut observe).await?;
        match result {
          ModelResult::Complete(response) => session.accept_response(response)?,
          ModelResult::Interrupted(partial) => session.accept_interruption(partial)?,
          ModelResult::Failed(outcome) => session.finish_run(outcome)?,
        }
      }
      SessionAction::ExecuteTools(calls) => {
        execute_tools(executor, session, &calls, control, &mut cursor, &mut observe).await?;
        session.complete_tools()?;
      }
    }
  }
}

fn notify_observers(
  session: &Session,
  cursor: &mut u64,
  observe: &mut impl FnMut(&SessionEvent),
) -> Result<(), SessionError> {
  let history = session.get_history();
  let end = history.len()?;
  while *cursor < end {
    let page = history.read_page(*cursor, crate::storage::PAGE_SIZE as usize)?;
    for record in &page.items {
      if let HistoryItem::Event(id) = record.item {
        let event = session
          .get_event(id)?
          .ok_or_else(|| crate::storage::StorageError::Corrupt("missing event".into()))?;
        // Stream events were already delivered live by receive_stream.
        if !matches!(&*event, SessionEvent::ModelStream(_)) {
          observe(&event);
        }
      }
    }
    *cursor += page.items.len() as u64;
  }
  Ok(())
}

enum ModelResult {
  Complete(Response),
  Interrupted(PartialResponse),
  Failed(RunOutcome),
}

async fn call_model(
  model_caller: &impl ModelCaller,
  request: &Request,
  session: &mut Session,
  control: &RunControl,
  cursor: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<ModelResult, SessionError> {
  let opened = {
    let opening = model_caller.call(request);
    let cancelled = control.wait_for_cancellation();
    pin_mut!(opening, cancelled);
    match select(cancelled, opening).await {
      Either::Left(_) => None,
      Either::Right((result, _)) => Some(result),
    }
  };
  Ok(match opened {
    None => finalize_interruption(StreamAccumulator::new()),
    Some(Err(error)) => ModelResult::Failed(RunOutcome::Failed(error)),
    Some(Ok(CallResponse::Complete(response))) => ModelResult::Complete(*response),
    Some(Ok(CallResponse::Stream(stream))) => {
      receive_stream(stream, session, control, cursor, observe).await?
    }
  })
}

fn finalize_interruption(accumulator: StreamAccumulator) -> ModelResult {
  match accumulator.finalize(StreamEnd::Interrupted { tools: ToolExecutionState::NotStarted }) {
    Ok(StreamFinalization::Incomplete(partial)) => ModelResult::Interrupted(*partial),
    Ok(StreamFinalization::Complete(_)) => unreachable!("interruption cannot complete a response"),
    Err(error) => ModelResult::Failed(RunOutcome::Failed(error)),
  }
}
fn finalize_failed_stream(accumulator: StreamAccumulator, error: Error) -> ModelResult {
  match accumulator.finalize(StreamEnd::Failed(error)) {
    Ok(StreamFinalization::Incomplete(partial)) => {
      ModelResult::Failed(RunOutcome::StreamFailed(partial))
    }
    Ok(StreamFinalization::Complete(_)) => unreachable!("failed stream cannot complete a response"),
    Err(error) => ModelResult::Failed(RunOutcome::Failed(error)),
  }
}

async fn receive_stream(
  mut stream: impl ModelStream,
  session: &mut Session,
  control: &RunControl,
  cursor: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<ModelResult, SessionError> {
  let mut accumulator = stream.create_accumulator();
  let mut batch = StreamEventBatch::default();
  let result = async {
    loop {
      let next = {
        let reading = stream.next();
        let cancelled = control.wait_for_cancellation();
        pin_mut!(reading, cancelled);
        // Keep the same read future across timer ticks. ModelStream::next need not be
        // cancellation-safe when a batch reaches its deadline.
        loop {
          let ready = select(cancelled.as_mut(), reading.as_mut());
          pin_mut!(ready);
          if let Some(deadline) = batch.deadline {
            let timer = tokio::time::sleep_until(deadline);
            pin_mut!(timer);
            match select(ready, timer).await {
              Either::Left((next, _)) => {
                break match next {
                  Either::Left(_) => None,
                  Either::Right((result, _)) => Some(result),
                };
              }
              Either::Right(_) => {
                batch.flush(session)?;
                notify_observers(session, cursor, observe)?;
              }
            }
          } else {
            break match ready.await {
              Either::Left(_) => None,
              Either::Right((result, _)) => Some(result),
            };
          }
        }
      };
      match next {
        None => return Ok(finalize_interruption(accumulator)),
        Some(Err(error)) => return Ok(finalize_failed_stream(accumulator, error)),
        Some(Ok(None)) => break,
        Some(Ok(Some(event))) => {
          if let Err(error) = accumulator.feed(event.clone()) {
            return Ok(finalize_failed_stream(accumulator, error));
          }
          let event = SessionEvent::ModelStream(event);
          batch.push(event.clone())?;
          observe(&event);
          if batch.is_full() {
            batch.flush(session)?;
            notify_observers(session, cursor, observe)?;
          }
        }
      }
    }
    if control.is_cancelled() {
      return Ok(finalize_interruption(accumulator));
    }
    Ok(match accumulator.finalize(StreamEnd::Complete) {
      Ok(StreamFinalization::Complete(response)) => ModelResult::Complete(*response),
      Ok(StreamFinalization::Incomplete(_)) => unreachable!("complete finalization is strict"),
      Err(error) => ModelResult::Failed(RunOutcome::Failed(error)),
    })
  }
  .await;
  stream.abort();
  // All completion, cancellation and protocol-error paths drain before the state changes.
  // A storage failure is returned to the caller, never reported as a successful shutdown.
  batch.flush(session)?;
  notify_observers(session, cursor, observe)?;
  result
}

#[derive(Default)]
struct StreamEventBatch {
  events: Vec<SessionEvent>,
  bytes: usize,
  deadline: Option<tokio::time::Instant>,
}
impl StreamEventBatch {
  fn push(&mut self, event: SessionEvent) -> Result<(), SessionError> {
    self.bytes = self.bytes.saturating_add(
      serde_json::to_vec(&event).map_err(crate::storage::StorageError::from)?.len(),
    );
    self
      .deadline
      .get_or_insert_with(|| tokio::time::Instant::now() + std::time::Duration::from_millis(100));
    self.events.push(event);
    Ok(())
  }
  fn is_full(&self) -> bool {
    self.events.len() >= 64
      || self.bytes >= 256 * 1024
      || self.deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
  }
  fn flush(&mut self, session: &mut Session) -> Result<(), SessionError> {
    session.record_events(std::mem::take(&mut self.events))?;
    self.bytes = 0;
    self.deadline = None;
    Ok(())
  }
}

async fn execute_tools(
  executor: &impl ToolExecutor,
  session: &mut Session,
  calls: &[ToolCall],
  control: &RunControl,
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
