use super::*;
use crate::session::statistics::{CallObservation, ModelCallPurpose, ModelCallStatus};

impl Session {
  pub fn get_model_calls(&self) -> ReadList<ModelCallRecord> {
    self
      .storage
      .open_list(self.record.model_calls.as_ref().expect("session initializes call list"))
      .read_only()
  }
  pub fn get_model_call(
    &self,
    id: ModelCallId,
  ) -> Result<Option<Arc<ModelCallRecord>>, SessionError> {
    Ok(self.get_model_calls().get(id.0)?)
  }
}
impl SessionEdit<'_, '_> {
  pub(super) fn start_model_call(
    &mut self,
    input_entry_count: u64,
    purpose: ModelCallPurpose,
  ) -> Result<(), SessionError> {
    let list = self.record.model_calls.as_ref().expect("session initializes call list");
    let id = ModelCallId(self.tx.list_len::<ModelCallRecord>(list)?);
    self.tx.append_item(
      list,
      &ModelCallRecord {
        id,
        purpose,
        generation: self.record.active,
        model: self.record.config.model.clone(),
        stream: purpose == ModelCallPurpose::Conversation && self.record.config.stream,
        input_entry_count,
        started_at: self.recorded_at,
        first_event_at: None,
        finished_at: None,
        elapsed_ms: None,
        status: ModelCallStatus::Running,
        usage: Default::default(),
        last_request_input_tokens: None,
        last_request_estimated_tokens: None,
        stop_reason: None,
      },
    )?;
    self.record.active_model_call = Some(id);
    Ok(())
  }
  pub(super) fn complete_model_call(
    &mut self,
    observation: CallObservation,
    status: ModelCallStatus,
  ) -> Result<(), SessionError> {
    let id = self
      .record
      .active_model_call
      .ok_or_else(|| StorageError::Corrupt("missing active model call".into()))?;
    let list = self.record.model_calls.as_ref().expect("session initializes call list");
    let mut call = (*self
      .tx
      .get_item::<ModelCallRecord>(list, id.0)?
      .ok_or_else(|| StorageError::Corrupt("missing model call record".into()))?)
    .clone();
    call.first_event_at = observation.first_event_at;
    call.finished_at = observation.finished_at;
    call.elapsed_ms = observation.elapsed_ms;
    call.usage = observation.usage;
    call.last_request_input_tokens = observation.last_request_input_tokens;
    call.last_request_estimated_tokens = observation.last_request_estimated_tokens;
    call.stop_reason = observation.stop_reason;
    call.status = status;
    self.tx.set_item(list, id.0, &call)?;
    Ok(())
  }
}
