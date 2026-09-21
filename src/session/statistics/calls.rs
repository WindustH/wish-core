use super::{ModelCallId, ModelCallRecord, Timestamp};
use crate::session::persistence::{SessionRecord, SessionTransaction};
use crate::session::statistics::{CallObservation, ModelCallPurpose, ModelCallStatus};
use crate::session::{Session, SessionError, SessionEvent};
use crate::storage::{ReadList, StorageError, Transaction};
use std::sync::Arc;

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
  pub(crate) fn start_compaction_call(&mut self) -> Result<(), SessionError> {
    self.update(|transaction| transaction.start_model_call(0, ModelCallPurpose::CompactionSummary))
  }
  pub(crate) fn complete_compaction_call(
    &mut self,
    observation: CallObservation,
    status: ModelCallStatus,
  ) -> Result<(), SessionError> {
    self.update(move |transaction| transaction.complete_model_call(observation, status))
  }
}
impl SessionTransaction<'_, '_> {
  pub(in crate::session) fn start_model_call(
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
  pub(in crate::session) fn complete_model_call(
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

pub(in crate::session) fn record_stream_usage(
  tx: &mut Transaction<'_>,
  record: &SessionRecord,
  timestamps: &[Timestamp],
  events: &[SessionEvent],
) -> Result<(), SessionError> {
  if let Some(id) = record.active_model_call {
    let list = record.model_calls.as_ref().expect("session initializes call list");
    let mut call = (*tx
      .get_item::<ModelCallRecord>(list, id.0)?
      .ok_or_else(|| StorageError::Corrupt("missing model call record".into()))?)
    .clone();
    for (timestamp, event) in timestamps.iter().zip(events) {
      if let SessionEvent::ModelStream(event) = event {
        call.first_event_at.get_or_insert(*timestamp);
        match event {
          crate::protocol::StreamEvent::Usage(usage) => call.usage = *usage,
          crate::protocol::StreamEvent::Stop(reason) => call.stop_reason = Some(*reason),
          _ => {}
        }
      }
    }
    tx.set_item(list, id.0, &call)?;
  }
  Ok(())
}
