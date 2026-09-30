//! A tool batch: the calls of one model turn and how each went, kept while its tools run. Once they
//! all have outcomes, the results join the context as entries and the batch is deleted.
use super::{SessionState, ToolExecution};
use crate::protocol::Message;
use crate::session::persistence::SessionTransaction;
use crate::session::{EntryOrigin, SessionError, SessionEvent, ToolCall, ToolOutcome};
use crate::storage::{ListId, StorageError};

impl SessionTransaction<'_, '_> {
  /// Starts a batch of `calls` for `turn`, none of them started.
  pub(super) fn start_tool_batch(
    &mut self,
    turn: usize,
    calls: Vec<ToolCall>,
  ) -> Result<(), SessionError> {
    let batch = self.allocate_list::<ToolExecution>()?;
    for call in calls {
      self.store.append_item(&batch, &ToolExecution { call, started: false, outcome: None })?;
    }
    self.transition_to(SessionState::ExecutingTools { turn, batch })
  }
  /// The calls of `batch` that have no outcome yet, in order.
  pub(super) fn pending_calls(&mut self, batch: &ListId) -> Result<Vec<ToolCall>, SessionError> {
    let executions = self.store.read_from::<ToolExecution>(batch, 0)?;
    Ok(
      executions
        .iter()
        .filter(|execution| execution.outcome.is_none())
        .map(|execution| execution.call.clone())
        .collect(),
    )
  }
  pub(super) fn mark_tool_started(&mut self, call: &ToolCall) -> Result<(), SessionError> {
    self.update_execution(call, |execution| execution.started = true)?;
    self.record_event(SessionEvent::ToolStarted(call.clone()))?;
    Ok(())
  }
  pub(super) fn record_tool_outcome(
    &mut self,
    call: &ToolCall,
    outcome: ToolOutcome,
  ) -> Result<(), SessionError> {
    let settled = outcome.clone();
    self.update_execution(call, |execution| execution.outcome = Some(settled))?;
    self.record_event(SessionEvent::ToolFinished { call: call.clone(), outcome })?;
    Ok(())
  }
  /// Applies `update` to the execution of `call` in the running batch.
  fn update_execution(
    &mut self,
    call: &ToolCall,
    update: impl FnOnce(&mut ToolExecution),
  ) -> Result<(), SessionError> {
    let SessionState::ExecutingTools { batch, .. } = self.record.state.clone() else {
      return Err(SessionError::UnexpectedPhase);
    };
    let executions = self.store.read_from::<ToolExecution>(&batch, 0)?;
    let position = executions
      .iter()
      .position(|execution| execution.call.call_id == call.call_id)
      .ok_or_else(|| StorageError::Corrupt("tool missing from active batch".into()))?;
    let mut execution = (*executions[position]).clone();
    update(&mut execution);
    self.store.set_item(&batch, position as u64, &execution)?;
    Ok(())
  }
  /// Settles the calls of `batch` that have no outcome: a started one may have had external
  /// effects before the run stopped, so its outcome is unknown; only one that never started is
  /// known to have done nothing.
  pub(super) fn cancel_unfinished(&mut self, batch: &ListId) -> Result<(), SessionError> {
    let executions = self.store.read_from::<ToolExecution>(batch, 0)?;
    for (position, execution) in executions.iter().enumerate() {
      if execution.outcome.is_none() {
        let mut settled = (**execution).clone();
        settled.outcome = Some(if execution.started {
          ToolOutcome::Unknown(
            "The run stopped while this tool was executing; its effects are unknown.".into(),
          )
        } else {
          ToolOutcome::Cancelled
        });
        self.store.set_item(batch, position as u64, &settled)?;
      }
    }
    Ok(())
  }
  /// Turns the settled `batch` into entries: every result in call order, then the input some of
  /// them carry, which keeps the results paired with their calls. The batch is deleted. Returns
  /// whether an outcome is unknown.
  pub(super) fn drain_batch(&mut self, batch: &ListId) -> Result<bool, SessionError> {
    let executions = self.store.read_from::<ToolExecution>(batch, 0)?;
    let mut unknown = false;
    let mut inputs = Vec::new();
    for execution in &executions {
      let outcome = execution
        .outcome
        .as_ref()
        .ok_or_else(|| StorageError::Corrupt("unsettled tool batch".into()))?;
      unknown |= matches!(outcome, ToolOutcome::Unknown(_));
      if let ToolOutcome::SuccessWithInput { input, .. } = outcome {
        inputs.push((execution.call.clone(), input.clone()));
      }
      let metadata = match outcome {
        ToolOutcome::SuccessWithMetadata { metadata, .. } => metadata.clone(),
        _ => Default::default(),
      };
      self.append_message(
        Message::ToolResult {
          metadata,
          call_id: execution.call.call_id.clone(),
          name: execution.call.name.clone(),
          content: outcome.encode_content(),
        },
        EntryOrigin::Tool,
      )?;
    }
    // The results are entries now; the batch only mattered while its tools ran.
    self.store.delete_list::<ToolExecution>(batch)?;
    for (call, content) in inputs {
      self.append_message(
        Message::User {
          metadata: serde_json::json!({"tool_call_id": call.call_id, "tool_name": call.name}),
          content,
        },
        EntryOrigin::Tool,
      )?;
    }
    Ok(unknown)
  }
}
