//! One logical model call: segments sent until one ends other than at the output limit, each
//! answered buffered or streamed. Stream events reach the observer live, renumbered across
//! segments; the call's observation is kept for its record. Nothing here touches the session.
use super::continuation::{Continuation, SegmentObservation, combine_usage};
use super::{CallResponse, ModelCaller, ModelStream};
use crate::executor::ExecutionControl;
use crate::session::{RunOutcome, SessionEvent, TokenEstimator, statistics::CallObservation};
use crate::{
  Error,
  protocol::{
    Request, Response, StopReason, StreamAccumulator, StreamEvent,
    model_use::stream::{PartialResponse, StreamEnd, StreamFinalization, ToolExecutionState},
  },
  utils::time::Timestamp,
};

/// How a logical model call ended.
pub(in crate::executor) enum ModelResult {
  Complete(Response),
  Interrupted(PartialResponse),
  Failed(RunOutcome),
}

/// Calls the model for `request`, continuing past output limits, and returns how the call ended
/// with its observation. `estimator` estimates each completed request, to calibrate later
/// estimates by; without one nothing is estimated.
pub(in crate::executor) async fn execute_model(
  model_caller: &impl ModelCaller,
  request: &Request,
  control: &ExecutionControl,
  estimator: Option<TokenEstimator>,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> (ModelResult, CallObservation) {
  let started = std::time::Instant::now();
  let mut continuation = Continuation::new(request);
  let mut first_event_at = None;
  let mut last_request_input_tokens = None;
  let mut last_request_estimated_tokens = None;
  loop {
    let mut observation = continuation.observe_next_segment();
    let mut result =
      call_segment(model_caller, &continuation.request, control, &mut observation, observe).await;
    first_event_at = first_event_at.or(observation.first_event_at);
    continuation.next_index = observation.next_index;
    let usage = combine_usage(continuation.usage, observation.usage);
    if matches!(result, SegmentResult::Complete(_) | SegmentResult::OutputLimited(_)) {
      last_request_input_tokens = observation.usage.input_tokens;
      last_request_estimated_tokens =
        estimator.map(|estimator| estimator.estimate_request(&continuation.request));
    }
    if let SegmentResult::OutputLimited(partial) = result {
      continuation.append_cut_output(partial.get_continuation_messages());
      continuation.usage = Some(usage);
      continue;
    }

    match &mut result {
      SegmentResult::Complete(response) => {
        continuation.messages.append(&mut response.messages);
        response.messages = continuation.messages;
        response.usage = usage;
      }
      SegmentResult::Interrupted(partial) => {
        partial.prepend_replay_messages(continuation.messages);
        partial.usage = usage;
      }
      SegmentResult::Failed(RunOutcome::StreamFailed(partial)) => partial.usage = usage,
      _ => {}
    }
    observation.call.first_event_at = first_event_at;
    observation.call.usage = usage;
    observation.call.last_request_input_tokens = last_request_input_tokens;
    observation.call.last_request_estimated_tokens = last_request_estimated_tokens;
    observation.call.finish(started);
    let result = match result {
      SegmentResult::Complete(response) => ModelResult::Complete(response),
      SegmentResult::Interrupted(partial) => ModelResult::Interrupted(partial),
      SegmentResult::Failed(outcome) => ModelResult::Failed(outcome),
      SegmentResult::OutputLimited(_) => unreachable!("handled within the model executor"),
    };
    return (result, observation.call);
  }
}

enum SegmentResult {
  Complete(Response),
  OutputLimited(PartialResponse),
  Interrupted(PartialResponse),
  Failed(RunOutcome),
}

async fn call_segment(
  model_caller: &impl ModelCaller,
  request: &Request,
  control: &ExecutionControl,
  observation: &mut SegmentObservation,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
) -> SegmentResult {
  match control.run_until_cancelled(model_caller.call(request)).await {
    None => finalize_interruption(StreamAccumulator::new()),
    Some(Err(error)) => SegmentResult::Failed(RunOutcome::Failed(error)),
    Some(Ok(CallResponse::Complete(response))) => {
      observation.usage = response.usage;
      observation.stop_reason = Some(response.stop_reason);
      if response.stop_reason == StopReason::MaxOutputLengthExceeded {
        match PartialResponse::from_output_limit(*response, model_caller.get_model_use_protocol()) {
          Ok(partial) => SegmentResult::OutputLimited(partial),
          Err(error) => SegmentResult::Failed(RunOutcome::Failed(error)),
        }
      } else {
        SegmentResult::Complete(*response)
      }
    }
    Some(Ok(CallResponse::Stream(stream))) => {
      read_segment_stream(stream, control, observe, observation).await
    }
  }
}

fn finalize_interruption(accumulator: StreamAccumulator) -> SegmentResult {
  match accumulator.finalize(StreamEnd::Interrupted { tools: ToolExecutionState::NotStarted }) {
    Ok(StreamFinalization::Incomplete(partial)) => SegmentResult::Interrupted(*partial),
    Ok(StreamFinalization::Complete(_)) => unreachable!("interruption cannot complete a response"),
    Err(error) => SegmentResult::Failed(RunOutcome::Failed(error)),
  }
}
fn finalize_failed_stream(accumulator: StreamAccumulator, error: Error) -> SegmentResult {
  match accumulator.finalize(StreamEnd::Failed(error)) {
    Ok(StreamFinalization::Incomplete(partial)) => {
      SegmentResult::Failed(RunOutcome::StreamFailed(partial))
    }
    Ok(StreamFinalization::Complete(_)) => unreachable!("failed stream cannot complete a response"),
    Err(error) => SegmentResult::Failed(RunOutcome::Failed(error)),
  }
}

async fn read_segment_stream(
  mut stream: impl ModelStream,
  control: &ExecutionControl,
  observe: &mut (impl FnMut(&SessionEvent) + Send),
  observation: &mut SegmentObservation,
) -> SegmentResult {
  let mut accumulator = stream.create_accumulator();
  let result = async {
    loop {
      match control.run_until_cancelled(stream.next()).await {
        None => return finalize_interruption(accumulator),
        Some(Err(error)) => return finalize_failed_stream(accumulator, error),
        Some(Ok(None)) => break,
        Some(Ok(Some(event))) => {
          observation.first_event_at.get_or_insert(Timestamp::now());
          match &event {
            StreamEvent::Usage(usage) => observation.usage = *usage,
            StreamEvent::Stop(reason) => observation.stop_reason = Some(*reason),
            _ => {}
          }
          if let Err(error) = accumulator.feed(event.clone()) {
            return finalize_failed_stream(accumulator, error);
          }
          let event = match observation.map_event(event) {
            Ok(Some(event)) => SessionEvent::ModelStream(event),
            Ok(None) => continue,
            Err(error) => return finalize_failed_stream(accumulator, error),
          };
          observe(&event);
        }
      }
    }
    if control.is_cancelled() {
      return finalize_interruption(accumulator);
    }
    if observation.stop_reason == Some(StopReason::MaxOutputLengthExceeded) {
      return match accumulator.finish_output_limit() {
        Ok(partial) => SegmentResult::OutputLimited(partial),
        Err(error) => SegmentResult::Failed(RunOutcome::Failed(error)),
      };
    }
    match accumulator.finalize(StreamEnd::Complete) {
      Ok(StreamFinalization::Complete(response)) => SegmentResult::Complete(*response),
      Ok(StreamFinalization::Incomplete(_)) => unreachable!("complete finalization is strict"),
      Err(error) => SegmentResult::Failed(RunOutcome::Failed(error)),
    }
  }
  .await;
  stream.abort();
  result
}
