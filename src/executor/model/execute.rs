use super::{CallResponse, ModelCaller, ModelStream};
use crate::executor::{ExecutionControl, observe::notify_observers};
use crate::session::{
  RunOutcome, Session, SessionError, SessionEvent,
  statistics::{CallObservation, Timestamp},
};
use crate::{
  Error,
  protocol::{
    Request, Response, StreamAccumulator,
    model_use::stream::{PartialResponse, StreamEnd, StreamFinalization, ToolExecutionState},
  },
};
use futures_util::{
  future::{Either, select},
  pin_mut,
};

pub(in crate::executor) async fn execute_model(
  model_caller: &impl ModelCaller,
  request: &Request,
  session: &mut Session,
  control: &ExecutionControl,
  cursor: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> Result<(ModelResult, CallObservation), SessionError> {
  let mut observation = CallObservation::default();
  let started = std::time::Instant::now();
  let result =
    call_model(model_caller, request, session, control, cursor, observe, &mut observation).await?;
  observation.finished_at = Some(Timestamp::now());
  observation.elapsed_ms = Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
  Ok((result, observation))
}

pub(in crate::executor) enum ModelResult {
  Complete(Response),
  Interrupted(PartialResponse),
  Failed(RunOutcome),
}

async fn call_model(
  model_caller: &impl ModelCaller,
  request: &Request,
  session: &mut Session,
  control: &ExecutionControl,
  cursor: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
  observation: &mut CallObservation,
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
    Some(Ok(CallResponse::Complete(response))) => {
      observation.usage = response.usage;
      observation.stop_reason = Some(response.stop_reason);
      ModelResult::Complete(*response)
    }
    Some(Ok(CallResponse::Stream(stream))) => {
      receive_stream(stream, session, control, cursor, observe, observation).await?
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
  control: &ExecutionControl,
  cursor: &mut u64,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
  observation: &mut CallObservation,
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
          let received_at = Timestamp::now();
          observation.first_event_at.get_or_insert(received_at);
          match &event {
            crate::protocol::StreamEvent::Usage(usage) => observation.usage = *usage,
            crate::protocol::StreamEvent::Stop(reason) => observation.stop_reason = Some(*reason),
            _ => {}
          }
          if let Err(error) = accumulator.feed(event.clone()) {
            return Ok(finalize_failed_stream(accumulator, error));
          }
          let event = SessionEvent::ModelStream(event);
          batch.push(received_at, event.clone())?;
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
  events: Vec<(Timestamp, SessionEvent)>,
  bytes: usize,
  deadline: Option<tokio::time::Instant>,
}
impl StreamEventBatch {
  fn push(&mut self, received_at: Timestamp, event: SessionEvent) -> Result<(), SessionError> {
    self.bytes = self.bytes.saturating_add(
      serde_json::to_vec(&event).map_err(crate::storage::StorageError::from)?.len(),
    );
    self
      .deadline
      .get_or_insert_with(|| tokio::time::Instant::now() + std::time::Duration::from_millis(100));
    self.events.push((received_at, event));
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
