//! Session state transitions; executor performs the actions selected here.
mod outcome;
mod state;
mod tool_batch;
pub use outcome::RunOutcome;
pub(crate) use state::SessionAction;
pub(in crate::session) use state::ToolExecution;
pub use state::{SessionPhase, SessionState};

use super::context::{is_input, is_valid_tool_use};
use super::persistence::SessionTransaction;
use super::statistics::{CallObservation, ModelCallPurpose, ModelCallRecord, ModelCallStatus};
use super::{
  Entry, EntryId, EntryOrigin, Session, SessionError, SessionEvent, ToolCall, ToolOutcome,
};
use crate::storage::StorageError;
use crate::{
  Error,
  protocol::{
    Message, Response,
    model_use::{response::StopReason, stream::PartialResponse},
  },
};
use std::{collections::HashSet, sync::Arc};

impl Session {
  pub(crate) fn advance(&mut self) -> Result<SessionAction, SessionError> {
    self.update(move |transaction| transaction.advance())
  }
  pub(crate) fn accept_response(
    &mut self,
    response: Response,
    observation: CallObservation,
  ) -> Result<(), SessionError> {
    self.end_model_call(observation, ModelCallStatus::Completed, |transaction| {
      transaction.accept_response(response)
    })
  }
  pub(crate) fn accept_interruption(
    &mut self,
    partial: PartialResponse,
    observation: CallObservation,
  ) -> Result<(), SessionError> {
    self.end_model_call(observation, ModelCallStatus::Interrupted, |transaction| {
      transaction.accept_interruption(partial)
    })
  }
  pub(crate) fn fail_model_call(
    &mut self,
    outcome: RunOutcome,
    observation: CallObservation,
  ) -> Result<(), SessionError> {
    self.end_model_call(observation, ModelCallStatus::Failed, |transaction| {
      transaction.finish_run(outcome)
    })
  }
  /// Ends the running conversation call as `status` and takes what it produced with `settle`, which
  /// still records under the call; the session has no running call after.
  fn end_model_call(
    &mut self,
    observation: CallObservation,
    status: ModelCallStatus,
    settle: impl FnOnce(&mut SessionTransaction<'_, '_>) -> Result<(), SessionError> + Send + 'static,
  ) -> Result<(), SessionError> {
    self.update(move |transaction| {
      transaction.complete_model_call(observation, status)?;
      settle(transaction)?;
      transaction.record.active_model_call = None;
      Ok(())
    })
  }
  pub(crate) fn start_tool(&mut self, call: &ToolCall) -> Result<(), SessionError> {
    let call = call.clone();
    self.update(move |transaction| transaction.mark_tool_started(&call))
  }
  pub(crate) fn accept_tool_outcome(
    &mut self,
    call: &ToolCall,
    outcome: ToolOutcome,
  ) -> Result<(), SessionError> {
    let call = call.clone();
    self.update(move |transaction| transaction.record_tool_outcome(&call, outcome))
  }
  pub(crate) fn complete_tools(&mut self) -> Result<(), SessionError> {
    self.update(move |transaction| transaction.complete_tools())
  }
  pub fn finish_run(&mut self, outcome: RunOutcome) -> Result<(), SessionError> {
    self.update(move |transaction| transaction.finish_run(outcome))
  }
  pub fn settle_interrupted(&mut self) -> Result<(), SessionError> {
    if self.record.state.is_stable() {
      return Ok(());
    }
    self.update(|transaction| transaction.settle_interrupted())
  }
  pub fn get_state(&self) -> &SessionState {
    &self.record.state
  }
  pub fn resume(&mut self) -> Result<(), SessionError> {
    self.require_stable()?;
    self.update(move |transaction| {
      transaction.transition_to(SessionState::Ready { completed_turns: 0, needs_model: true })
    })
  }
  pub(crate) fn require_stable(&self) -> Result<(), SessionError> {
    if self.record.state.is_stable() { Ok(()) } else { Err(SessionError::Busy) }
  }
  pub(crate) fn begin_compaction(&mut self) -> Result<(), SessionError> {
    self.require_stable()?;
    self.update(|transaction| {
      let resume = Box::new(transaction.record.state.clone());
      transaction.transition_to(SessionState::Compacting { resume })
    })
  }
  pub(crate) fn end_compaction(&mut self) -> Result<(), SessionError> {
    self.update(|transaction| {
      let SessionState::Compacting { resume } = transaction.record.state.clone() else {
        return Err(SessionError::UnexpectedPhase);
      };
      transaction.record.active_model_call = None;
      transaction.transition_to(*resume)
    })
  }
}
impl SessionTransaction<'_, '_> {
  fn advance(&mut self) -> Result<SessionAction, SessionError> {
    match self.record.state.clone() {
      SessionState::Idle => Ok(SessionAction::Finished(RunOutcome::Completed)),
      SessionState::Suspended { outcome } => match self.load_event(outcome)?.as_ref() {
        SessionEvent::Finished(outcome) => Ok(SessionAction::Finished(outcome.clone())),
        _ => Err(StorageError::Corrupt("invalid run outcome reference".into()).into()),
      },
      SessionState::Ready { completed_turns, needs_model } => {
        self.start_turn(completed_turns, needs_model)
      }
      SessionState::CallingModel { .. } | SessionState::Compacting { .. } => {
        Err(SessionError::UnexpectedPhase)
      }
      SessionState::ExecutingTools { batch, .. } => {
        Ok(SessionAction::ExecuteTools(self.pending_calls(&batch)?))
      }
    }
  }
  /// Starts the turn after `completed_turns`: the queued input joins the context and the model is
  /// called with it. With no input and no model call due, the run finishes instead.
  fn start_turn(
    &mut self,
    completed_turns: usize,
    needs_model: bool,
  ) -> Result<SessionAction, SessionError> {
    let queue_end = self.store.list_len::<EntryId>(&self.record.queue)?;
    if !needs_model && self.record.queue_head == queue_end {
      self.finish_run(RunOutcome::Completed)?;
      return Ok(SessionAction::Finished(RunOutcome::Completed));
    }
    let turn = completed_turns + 1;
    self.consume_inputs(queue_end)?;
    let request = Arc::new(self.build_request()?);
    self.start_model_call(ModelCallPurpose::Conversation, request.conversation.len() as u64)?;
    self.transition_to(SessionState::CallingModel { turn })?;
    self.record_event(SessionEvent::TurnStarted { turn })?;
    Ok(SessionAction::CallModel(request))
  }
  fn accept_response(&mut self, response: Response) -> Result<(), SessionError> {
    let SessionState::CallingModel { turn } = self.record.state else {
      return Err(SessionError::UnexpectedPhase);
    };
    if !matches!(response.stop_reason, StopReason::Stop | StopReason::ToolUse) {
      return self.finish_run(RunOutcome::ModelStopped(Box::new(response)));
    }
    let calls = tool_calls(&response);
    if !is_acceptable(&response, &calls) {
      self.record_event(SessionEvent::ResponseRejected(Box::new(response)))?;
      return self.finish_run(RunOutcome::Failed(Error::Malformed(
        "invalid agent response or tool call batch".into(),
      )));
    }
    let entry_start = self.store.list_len::<Entry>(&self.record.entries)?;
    for message in response.messages {
      self.append_message(message, EntryOrigin::Model)?;
    }
    let entry_end = self.store.list_len::<Entry>(&self.record.entries)?;
    self.record_event(SessionEvent::ResponseAccepted {
      turn,
      entry_start,
      entry_end,
      stop_reason: response.stop_reason,
      usage: response.usage,
      account_state: response.account_state,
    })?;
    if calls.is_empty() {
      self.complete_turn(turn, false)
    } else {
      self.start_tool_batch(turn, calls)
    }
  }
  fn accept_interruption(&mut self, partial: PartialResponse) -> Result<(), SessionError> {
    let SessionState::CallingModel { turn } = self.record.state else {
      return Err(SessionError::UnexpectedPhase);
    };
    for message in partial.get_replay_messages() {
      self.append_message(message.clone(), EntryOrigin::Interrupted)?;
    }
    self.record_event(SessionEvent::ResponseInterrupted(Box::new(partial)))?;
    self.record_event(SessionEvent::StableBoundary { turn })?;
    self.finish_run(RunOutcome::Interrupted)
  }
  fn complete_tools(&mut self) -> Result<(), SessionError> {
    let SessionState::ExecutingTools { turn, batch } = self.record.state.clone() else {
      return Err(SessionError::UnexpectedPhase);
    };
    if self.drain_batch(&batch)? {
      self.record_event(SessionEvent::StableBoundary { turn })?;
      self.finish_run(RunOutcome::ToolOutcomeUnknown)
    } else {
      self.complete_turn(turn, true)
    }
  }
  fn complete_turn(&mut self, turn: usize, needs_model: bool) -> Result<(), SessionError> {
    self.record_event(SessionEvent::StableBoundary { turn })?;
    self.transition_to(SessionState::Ready { completed_turns: turn, needs_model })
  }
  /// Ends the run with `outcome`: idle when it completed, otherwise suspended on the `Finished`
  /// event that holds the outcome. A call still running ends with it.
  fn finish_run(&mut self, outcome: RunOutcome) -> Result<(), SessionError> {
    self.abandon_running_call(&outcome)?;
    if matches!(outcome, RunOutcome::Completed) {
      self.transition_to(SessionState::Idle)?;
      self.record_event(SessionEvent::Finished(outcome))?;
    } else {
      // The suspended state names the outcome's event, which follows the change of phase.
      let from = self.record.state.get_phase();
      self.record_event(SessionEvent::StateChanged { from, to: SessionPhase::Suspended })?;
      let outcome = self.record_event(SessionEvent::Finished(outcome))?;
      self.record.state = SessionState::Suspended { outcome };
    }
    Ok(())
  }
  /// Ends the running call, if its record is still running, as the run's `outcome` ends it.
  fn abandon_running_call(&mut self, outcome: &RunOutcome) -> Result<(), SessionError> {
    let Some(id) = self.record.active_model_call.take() else { return Ok(()) };
    let list = &self.record.model_calls;
    if let Some(call) = self.store.get_item::<ModelCallRecord>(list, id.0)?
      && call.status == ModelCallStatus::Running
    {
      let mut call = (*call).clone();
      let status = if matches!(outcome, RunOutcome::Interrupted) {
        ModelCallStatus::Interrupted
      } else {
        ModelCallStatus::Failed
      };
      call.abandon(status, self.recorded_at);
      self.store.set_item(list, id.0, &call)?;
    }
    Ok(())
  }
  pub(in crate::session) fn settle_interrupted(&mut self) -> Result<(), SessionError> {
    match self.record.state.clone() {
      SessionState::Idle | SessionState::Ready { .. } | SessionState::Suspended { .. } => Ok(()),
      SessionState::CallingModel { turn } => {
        self.record_event(SessionEvent::StableBoundary { turn })?;
        self.finish_run(RunOutcome::Interrupted)
      }
      SessionState::ExecutingTools { batch, .. } => {
        self.cancel_unfinished(&batch)?;
        self.complete_tools()?;
        if !matches!(self.record.state, SessionState::Suspended { .. }) {
          self.finish_run(RunOutcome::Interrupted)?;
        }
        Ok(())
      }
      SessionState::Compacting { resume } => {
        self.transition_to(*resume)?;
        if !self.record.state.is_stable() {
          self.settle_interrupted()?;
        } else if !matches!(self.record.state, SessionState::Suspended { .. }) {
          self.finish_run(RunOutcome::Interrupted)?;
        }
        Ok(())
      }
    }
  }
  pub fn transition_to(&mut self, next: SessionState) -> Result<(), SessionError> {
    let from = self.record.state.get_phase();
    let to = next.get_phase();
    self.record.state = next;
    self.record_event(SessionEvent::StateChanged { from, to })?;
    Ok(())
  }
}

/// The tool calls a response asks for, in order.
fn tool_calls(response: &Response) -> Vec<ToolCall> {
  response
    .messages
    .iter()
    .filter_map(|message| match message {
      Message::ToolUse { call_id, name, arguments, .. } => Some(ToolCall {
        call_id: call_id.clone(),
        name: name.clone(),
        arguments: arguments.clone(),
      }),
      _ => None,
    })
    .collect()
}

/// Whether a response the model finished can join the context: it asks for tools exactly when it
/// stopped for them, each call is well formed with an id of its own, and it holds only what a model
/// writes.
fn is_acceptable(response: &Response, calls: &[ToolCall]) -> bool {
  let mut ids = HashSet::new();
  (response.stop_reason == StopReason::ToolUse) != calls.is_empty()
    && calls.iter().all(|call| {
      is_valid_tool_use(&call.call_id, &call.name, &call.arguments) && ids.insert(&call.call_id)
    })
    && !response
      .messages
      .iter()
      .any(|message| is_input(message) || matches!(message, Message::ToolResult { .. }))
}
