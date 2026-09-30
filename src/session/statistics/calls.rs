use super::{ModelCallId, ModelCallRecord};
use crate::session::persistence::SessionTransaction;
use crate::session::statistics::{CallObservation, ModelCallPurpose, ModelCallStatus};
use crate::session::{GenerationId, Session, SessionError};
use crate::storage::StorageError;
use crate::utils::time::Timestamp;

impl ModelCallRecord {
  /// A call of `model` started at `started_at`, running and observed nothing yet.
  fn new(
    id: ModelCallId,
    purpose: ModelCallPurpose,
    generation: GenerationId,
    model: String,
    stream: bool,
    input_entry_count: u64,
    started_at: Timestamp,
  ) -> Self {
    Self {
      id,
      purpose,
      generation,
      model,
      stream,
      input_entry_count,
      started_at,
      first_event_at: None,
      finished_at: None,
      elapsed_ms: None,
      status: ModelCallStatus::Running,
      usage: Default::default(),
      last_request_input_tokens: None,
      last_request_estimated_tokens: None,
      stop_reason: None,
    }
  }
  /// Ends the call as `status`, with what the caller observed of it.
  fn finish(&mut self, status: ModelCallStatus, observation: CallObservation) {
    self.first_event_at = observation.first_event_at;
    self.finished_at = observation.finished_at;
    self.elapsed_ms = observation.elapsed_ms;
    self.usage = observation.usage;
    self.last_request_input_tokens = observation.last_request_input_tokens;
    self.last_request_estimated_tokens = observation.last_request_estimated_tokens;
    self.stop_reason = observation.stop_reason;
    self.status = status;
  }
  /// Ends, at `at`, a call whose run stopped without its observation.
  pub(in crate::session) fn abandon(&mut self, status: ModelCallStatus, at: Timestamp) {
    self.status = status;
    self.finished_at = Some(at);
    self.elapsed_ms = Some(at.0.saturating_sub(self.started_at.0));
  }
}

impl Session {
  /// Starts the record of a side call, which becomes the call the session's entries and events
  /// belong to until it completes.
  pub(crate) fn start_model_call(
    &mut self,
    purpose: ModelCallPurpose,
    input_entry_count: u64,
  ) -> Result<(), SessionError> {
    self.update(move |transaction| transaction.start_model_call(purpose, input_entry_count))
  }
  pub(crate) fn complete_compaction_call(
    &mut self,
    observation: CallObservation,
    status: ModelCallStatus,
  ) -> Result<(), SessionError> {
    self.update(move |transaction| transaction.complete_model_call(observation, status))
  }
  /// Records a finished standby summary call and returns its ID, which the summary it produced
  /// then carries.
  pub(crate) fn record_completed_compaction_call(
    &mut self,
    observation: CallObservation,
    input_entry_count: u64,
  ) -> Result<ModelCallId, SessionError> {
    self.update(move |transaction| {
      let started_at = match (observation.finished_at, observation.elapsed_ms) {
        (Some(finished), Some(elapsed)) => Timestamp(finished.0.saturating_sub(elapsed)),
        _ => transaction.recorded_at,
      };
      let mut call = transaction.new_model_call(
        ModelCallPurpose::CompactionSummary,
        false,
        input_entry_count,
        started_at,
      )?;
      call.finish(ModelCallStatus::Completed, observation);
      transaction.store.append_item(&transaction.record.model_calls, &call)?;
      Ok(call.id)
    })
  }
}
impl SessionTransaction<'_, '_> {
  /// Starts the record of a call, made with the session's model, which the session's entries and
  /// events then belong to.
  pub(in crate::session) fn start_model_call(
    &mut self,
    purpose: ModelCallPurpose,
    input_entry_count: u64,
  ) -> Result<(), SessionError> {
    let stream = match purpose {
      ModelCallPurpose::Conversation => self.record.config.stream,
      ModelCallPurpose::CompactionTranslation => true,
      ModelCallPurpose::CompactionSummary | ModelCallPurpose::UpstreamCompaction => false,
    };
    let call = self.new_model_call(purpose, stream, input_entry_count, self.recorded_at)?;
    self.store.append_item(&self.record.model_calls, &call)?;
    self.record.active_model_call = Some(call.id);
    Ok(())
  }
  /// A record for the next call, in the active generation with the session's model.
  fn new_model_call(
    &mut self,
    purpose: ModelCallPurpose,
    stream: bool,
    input_entry_count: u64,
    started_at: Timestamp,
  ) -> Result<ModelCallRecord, SessionError> {
    let id = ModelCallId(self.store.list_len::<ModelCallRecord>(&self.record.model_calls)?);
    Ok(ModelCallRecord::new(
      id,
      purpose,
      self.record.active,
      self.record.config.model.clone(),
      stream,
      input_entry_count,
      started_at,
    ))
  }
  /// Ends the running call as `status`, with what the caller observed of it.
  pub(in crate::session) fn complete_model_call(
    &mut self,
    observation: CallObservation,
    status: ModelCallStatus,
  ) -> Result<(), SessionError> {
    let id = self
      .record
      .active_model_call
      .ok_or_else(|| StorageError::Corrupt("missing active model call".into()))?;
    let list = &self.record.model_calls;
    let mut call = (*self
      .store
      .get_item::<ModelCallRecord>(list, id.0)?
      .ok_or_else(|| StorageError::Corrupt("missing model call record".into()))?)
    .clone();
    call.finish(status, observation);
    self.store.set_item(list, id.0, &call)?;
    Ok(())
  }
}
