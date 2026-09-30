//! Read access to a session's stored lists, for the owner and for any other task.
use super::persistence::SessionRecord;
use super::statistics::ModelCallRecord;
use super::{
  Entry, EntryId, EventId, Generation, GenerationId, HistoryRecord, Session, SessionError,
  SessionEvent,
};
use crate::storage::{ListId, ReadList, Storage, StorageError};
use std::sync::Arc;

/// A cloneable, session-scoped reader. It never claims the running owner or changes the session,
/// so it can be kept while an executor owns the session, across generation switches, and after the
/// owner is gone. Each read is a transaction of its own and sees what was committed. It holds the
/// session's lists, which are fixed when the session is created; the header's other fields, such as
/// the state and which generation is active, are the owner's to read.
#[derive(Clone)]
pub struct SessionReader {
  pub(super) storage: Storage,
  pub(super) entries: ListId,
  pub(super) events: ListId,
  pub(super) history: ListId,
  generations: ListId,
  queue: ListId,
  model_calls: ListId,
}
impl SessionReader {
  pub(super) fn new(storage: Storage, record: &SessionRecord) -> Self {
    Self {
      storage,
      entries: record.entries.clone(),
      events: record.events.clone(),
      history: record.history.clone(),
      generations: record.generations.clone(),
      queue: record.queue.clone(),
      model_calls: record.model_calls.clone(),
    }
  }
  pub fn get_history(&self) -> ReadList<HistoryRecord> {
    self.storage.open_list(&self.history)
  }
  pub fn get_entries(&self) -> ReadList<Entry> {
    self.storage.open_list(&self.entries)
  }
  pub fn get_events(&self) -> ReadList<SessionEvent> {
    self.storage.open_list(&self.events)
  }
  pub fn get_entry(&self, id: EntryId) -> Result<Option<Arc<Entry>>, SessionError> {
    Ok(self.get_entries().get(id.0 as u64)?)
  }
  pub fn get_event(&self, id: EventId) -> Result<Option<Arc<SessionEvent>>, SessionError> {
    Ok(self.get_events().get(id.0)?)
  }
  pub fn get_generations(&self) -> ReadList<Generation> {
    self.storage.open_list(&self.generations)
  }
  pub fn get_generation(&self, id: GenerationId) -> Result<Arc<Generation>, SessionError> {
    Ok(
      self
        .get_generations()
        .get(id.0 as u64)?
        .ok_or_else(|| StorageError::Corrupt("missing generation".into()))?,
    )
  }
  /// The entries a generation's context holds, in order.
  pub fn get_generation_entry_ids(
    &self,
    id: GenerationId,
  ) -> Result<ReadList<EntryId>, SessionError> {
    Ok(self.storage.open_list(&self.get_generation(id)?.entries))
  }
  /// Every entry of a generation, in order.
  pub fn read_generation_entry_ids(&self, id: GenerationId) -> Result<Vec<EntryId>, SessionError> {
    Ok(self.get_generation_entry_ids(id)?.read_all()?.iter().map(|id| **id).collect())
  }
  pub fn get_model_calls(&self) -> ReadList<ModelCallRecord> {
    self.storage.open_list(&self.model_calls)
  }
  /// The input queue's log. Positions below the owner's queue head have been consumed.
  pub fn get_message_queue(&self) -> ReadList<EntryId> {
    self.storage.open_list(&self.queue)
  }
}

impl Session {
  /// Reads the session's lists; clone it for a task of its own.
  pub fn reader(&self) -> &SessionReader {
    &self.reader
  }
  pub fn get_active_generation(&self) -> Result<Arc<Generation>, SessionError> {
    self.reader.get_generation(self.record.active)
  }
  pub fn get_standby_generation(&self) -> Result<Arc<Generation>, SessionError> {
    self.reader.get_generation(self.record.standby)
  }
}
