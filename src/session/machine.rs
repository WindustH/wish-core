use super::*;
use super::{edit::SessionEdit, state::SessionAction};
use crate::{
  Error,
  agent::{ToolCall, ToolOutcome},
  protocol::{
    Response,
    model_use::{response::StopReason, stream::PartialResponse},
  },
  storage::PAGE_SIZE,
};
use std::{collections::HashSet, sync::Arc};

impl Session {
  pub(crate) fn advance(&mut self) -> Result<SessionAction, SessionError> {
    self.update(move |edit| edit.advance())
  }
  pub(crate) fn accept_response(&mut self, response: Response) -> Result<(), SessionError> {
    self.update(move |edit| edit.accept_response(response))
  }
  pub(crate) fn accept_interruption(
    &mut self,
    partial: PartialResponse,
  ) -> Result<(), SessionError> {
    self.update(move |edit| edit.accept_interruption(partial))
  }
  pub(crate) fn start_tool(&mut self, call: &ToolCall) -> Result<(), SessionError> {
    let call = call.clone();
    self.update(move |edit| edit.update_tool(&call, None))
  }
  pub(crate) fn accept_tool_outcome(
    &mut self,
    call: &ToolCall,
    outcome: ToolOutcome,
  ) -> Result<(), SessionError> {
    let call = call.clone();
    self.update(move |edit| edit.update_tool(&call, Some(outcome)))
  }
  pub(crate) fn complete_tools(&mut self) -> Result<(), SessionError> {
    self.update(move |edit| edit.complete_tools())
  }
  pub(crate) fn finish_run(&mut self, outcome: RunOutcome) -> Result<(), SessionError> {
    self.update(move |edit| edit.finish_run(outcome))
  }
}
impl SessionEdit<'_, '_> {
  fn advance(&mut self) -> Result<SessionAction, SessionError> {
    match self.record.state.clone() {
      SessionState::Idle => Ok(SessionAction::Finished(RunOutcome::Completed)),
      SessionState::Suspended { outcome } => {
        let event = self
          .tx
          .get_item::<SessionEvent>(&self.record.events, outcome.0)?
          .ok_or_else(|| StorageError::Corrupt("missing run outcome".into()))?;
        if let SessionEvent::Finished(outcome) = event.as_ref() {
          Ok(SessionAction::Finished(outcome.clone()))
        } else {
          Err(StorageError::Corrupt("invalid run outcome reference".into()).into())
        }
      }
      SessionState::Ready { completed_turns, needs_model } => {
        let queue_end = self.tx.list_len::<EntryId>(&self.record.queue)?;
        if !needs_model && self.record.queue_head == queue_end {
          self.finish_run(RunOutcome::Completed)?;
          return Ok(SessionAction::Finished(RunOutcome::Completed));
        }
        if completed_turns >= self.record.config.run.max_turns {
          self.finish_run(RunOutcome::TurnLimit)?;
          return Ok(SessionAction::Finished(RunOutcome::TurnLimit));
        }
        let turn = completed_turns + 1;
        let queue_start = self.record.queue_head;
        let generation = self.load_generation(self.record.active)?;
        while self.record.queue_head < queue_end {
          let page = self.tx.read_page::<EntryId>(
            &self.record.queue,
            self.record.queue_head,
            PAGE_SIZE as usize,
          )?;
          for id in &page.items {
            self.tx.append_item(&generation.entries, id.as_ref())?;
            self.record_history(HistoryItem::Message(**id))?;
          }
          self.record.queue_head += page.items.len() as u64;
        }
        if queue_start != queue_end {
          self.record_event(SessionEvent::InputsConsumed { queue_start, queue_end })?;
        }
        let request = Arc::new(self.build_request()?);
        self.transition_to(SessionState::CallingModel { turn })?;
        self.record_event(SessionEvent::TurnStarted { turn })?;
        Ok(SessionAction::CallModel(request))
      }
      SessionState::CallingModel { .. } => Err(SessionError::Busy),
      SessionState::ExecutingTools { batch, .. } => {
        let mut calls = Vec::new();
        let length = self.tx.list_len::<ToolExecution>(&batch)?;
        let mut start = 0;
        while start < length {
          let page = self.tx.read_page::<ToolExecution>(&batch, start, PAGE_SIZE as usize)?;
          calls.extend(
            page.items.iter().filter(|item| item.outcome.is_none()).map(|item| item.call.clone()),
          );
          start += page.items.len() as u64;
        }
        Ok(SessionAction::ExecuteTools(calls))
      }
    }
  }
  fn accept_response(&mut self, response: Response) -> Result<(), SessionError> {
    let SessionState::CallingModel { turn } = self.record.state else {
      return Err(SessionError::Busy);
    };
    if !matches!(response.stop_reason, StopReason::Stop | StopReason::ToolUse) {
      return self.finish_run(RunOutcome::ModelStopped(Box::new(response)));
    }
    let calls: Vec<_> = response
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
      .collect();
    let mut ids = HashSet::new();
    if (response.stop_reason == StopReason::ToolUse) == calls.is_empty()
      || calls.iter().any(|call| {
        call.call_id.is_empty()
          || call.name.is_empty()
          || !call.arguments.is_object()
          || !ids.insert(&call.call_id)
      })
      || response.messages.iter().any(|message| {
        matches!(
          message,
          Message::User { .. }
            | Message::System { .. }
            | Message::Developer { .. }
            | Message::ToolResult { .. }
        )
      })
    {
      self.record_event(SessionEvent::ResponseRejected(Box::new(response)))?;
      return self.finish_run(RunOutcome::Failed(Error::Malformed(
        "invalid agent response or tool call batch".into(),
      )));
    }
    let entry_start = self.tx.list_len::<Entry>(&self.record.entries)?;
    for message in response.messages {
      self.append_message(message, EntryOrigin::Model)?;
    }
    let entry_end = self.tx.list_len::<Entry>(&self.record.entries)?;
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
      let batch = self.create_list::<ToolExecution>()?;
      for call in calls {
        self.tx.append_item(&batch, &ToolExecution { call, started: false, outcome: None })?;
      }
      self.transition_to(SessionState::ExecutingTools { turn, batch })
    }
  }
  fn accept_interruption(&mut self, partial: PartialResponse) -> Result<(), SessionError> {
    let SessionState::CallingModel { turn } = self.record.state else {
      return Err(SessionError::Busy);
    };
    for message in partial.get_replay_messages() {
      self.append_message(message.clone(), EntryOrigin::Interrupted)?;
    }
    self.record_event(SessionEvent::ResponseInterrupted(Box::new(partial)))?;
    self.record_event(SessionEvent::StableBoundary { turn })?;
    self.finish_run(RunOutcome::Interrupted)
  }
  fn update_tool(
    &mut self,
    call: &ToolCall,
    outcome: Option<ToolOutcome>,
  ) -> Result<(), SessionError> {
    let SessionState::ExecutingTools { batch, .. } = self.record.state.clone() else {
      return Err(SessionError::Busy);
    };
    let length = self.tx.list_len::<ToolExecution>(&batch)?;
    let mut start = 0;
    while start < length {
      let page = self.tx.read_page::<ToolExecution>(&batch, start, PAGE_SIZE as usize)?;
      for (offset, item) in page.items.iter().enumerate() {
        if item.call.call_id == call.call_id {
          let mut item = (**item).clone();
          if let Some(outcome) = outcome {
            item.outcome = Some(outcome.clone());
            self.tx.set_item(&batch, start + offset as u64, &item)?;
            return self.record_event(SessionEvent::ToolFinished { call: call.clone(), outcome });
          } else {
            item.started = true;
            self.tx.set_item(&batch, start + offset as u64, &item)?;
            return self.record_event(SessionEvent::ToolStarted(call.clone()));
          }
        }
      }
      start += page.items.len() as u64;
    }
    Err(StorageError::Corrupt("tool missing from active batch".into()).into())
  }
  fn complete_tools(&mut self) -> Result<(), SessionError> {
    let SessionState::ExecutingTools { turn, batch } = self.record.state.clone() else {
      return Err(SessionError::Busy);
    };
    let length = self.tx.list_len::<ToolExecution>(&batch)?;
    let mut start = 0;
    let mut unknown = false;
    while start < length {
      let page = self.tx.read_page::<ToolExecution>(&batch, start, PAGE_SIZE as usize)?;
      for execution in &page.items {
        let outcome = execution
          .outcome
          .as_ref()
          .ok_or_else(|| StorageError::Corrupt("unsettled tool batch".into()))?;
        unknown |= matches!(outcome, ToolOutcome::Unknown(_));
        self.append_message(
          Message::ToolResult {
            metadata: Default::default(),
            call_id: execution.call.call_id.clone(),
            name: execution.call.name.clone(),
            content: outcome.encode_content(),
          },
          EntryOrigin::Tool,
        )?;
      }
      start += page.items.len() as u64;
    }
    if unknown {
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
  fn finish_run(&mut self, outcome: RunOutcome) -> Result<(), SessionError> {
    if matches!(outcome, RunOutcome::Completed) {
      self.transition_to(SessionState::Idle)?;
    } else {
      let event = EventId(self.tx.list_len::<SessionEvent>(&self.record.events)? + 1);
      self.transition_to(SessionState::Suspended { outcome: event })?;
    }
    self.record_event(SessionEvent::Finished(outcome))
  }
}
