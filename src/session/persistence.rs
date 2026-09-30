//! Session record layout and transaction boundaries; database/cache mechanics live in storage.
use super::history::HistoryRecord;
use super::machine::ToolExecution;
use super::statistics::{ModelCallId, ModelCallRecord};
use super::{
  Entry, EntryId, EventId, Generation, GenerationId, Session, SessionConfig, SessionError,
  SessionEvent, SessionSender, SessionState,
};
use crate::storage::{ListId, StorageError, StoredValue, Transaction};
use crate::utils::time::Timestamp;
use serde_json::Value;
use std::sync::Arc;

/// The session header: settings, state, and the lists everything else is kept in. Only the owner
/// changes it; see [`SessionSender::transact`].
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionRecord {
  pub(super) metadata: Value,
  pub(super) config: SessionConfig,
  pub(super) state: SessionState,
  pub(super) active: GenerationId,
  pub(super) standby: GenerationId,
  pub(super) entries: ListId,
  pub(super) events: ListId,
  pub(super) history: ListId,
  pub(super) generations: ListId,
  pub(super) queue: ListId,
  pub(super) queue_head: u64,
  pub(super) next_list_id: u64,
  pub(super) model_calls: ListId,
  pub(super) active_model_call: Option<ModelCallId>,
}

/// The kind each stored session type is kept under. These strings are part of the database format:
/// they are the paths the types had when their values were first written, and they stay as they are
/// wherever the types move.
macro_rules! stored_kinds {
  ($($type:ty => $kind:literal,)*) => {
    $(impl StoredValue for $type {
      const KIND: &'static str = $kind;
    })*
  };
}
stored_kinds! {
  SessionRecord => "wish_core::session::persistence::SessionRecord",
  Entry => "wish_core::session::history::entry::Entry",
  EntryId => "wish_core::session::history::entry::EntryId",
  SessionEvent => "wish_core::session::history::event::SessionEvent",
  HistoryRecord => "wish_core::session::history::HistoryRecord",
  Generation => "wish_core::session::context::generation::Generation",
  ModelCallRecord => "wish_core::session::statistics::ModelCallRecord",
  ToolExecution => "wish_core::session::machine::state::ToolExecution",
}

/// One storage transaction on a session: its record, and the time the transaction's facts are
/// recorded at.
pub(super) struct SessionTransaction<'a, 'db> {
  pub record: &'a mut SessionRecord,
  pub store: &'a mut Transaction<'db>,
  pub key: &'a str,
  pub recorded_at: Timestamp,
}
impl Session {
  /// Runs `apply` in one transaction and saves the record it leaves.
  pub(in crate::session) fn update<R: Send + 'static>(
    &mut self,
    apply: impl FnOnce(&mut SessionTransaction<'_, '_>) -> Result<R, SessionError> + Send + 'static,
  ) -> Result<R, SessionError> {
    let mut record = self.record.clone();
    let key = self.key.clone();
    let recorded_at = Timestamp::now();
    let (result, record) = self.storage.transaction(move |store| -> Result<_, SessionError> {
      let result =
        apply(&mut SessionTransaction { record: &mut record, store, key: &key, recorded_at })?;
      store.save_object(&key, &record)?;
      Ok((result, record))
    })?;
    self.record = record;
    Ok(result)
  }
  /// Runs `read` on the session as committed. The transaction is rolled back, so reading never
  /// changes anything.
  pub(in crate::session) fn read<R: Send + 'static>(
    &self,
    read: impl FnOnce(&mut SessionTransaction<'_, '_>) -> Result<R, SessionError> + Send + 'static,
  ) -> Result<R, SessionError> {
    let mut record = self.record.clone();
    let key = self.key.clone();
    let recorded_at = Timestamp::now();
    self.storage.rehearse(move |store| {
      read(&mut SessionTransaction { record: &mut record, store, key: &key, recorded_at })
    })
  }
}
impl SessionSender {
  /// Runs `apply` in one transaction on the session as stored, from beside the owner. The owner
  /// saves its whole record with each change it makes, so a change a sender made to the record would
  /// be lost: `apply` may append to and edit the session's lists, and a transaction that changed
  /// the record is refused and rolled back.
  pub(in crate::session) fn transact<R: Send + 'static>(
    &self,
    apply: impl FnOnce(&mut SessionTransaction<'_, '_>) -> Result<R, SessionError> + Send + 'static,
  ) -> Result<R, SessionError> {
    let key = self.key.clone();
    let recorded_at = Timestamp::now();
    self.storage.transaction(move |store| {
      let stored = store.load_object::<SessionRecord>(&key)?;
      let mut record = (*stored).clone();
      let result =
        apply(&mut SessionTransaction { record: &mut record, store, key: &key, recorded_at })?;
      if serde_json::to_value(&record).map_err(StorageError::from)?
        != serde_json::to_value(&*stored).map_err(StorageError::from)?
      {
        return Err(StorageError::Corrupt("a sender changed the session record".into()).into());
      }
      Ok(result)
    })
  }
}
impl SessionTransaction<'_, '_> {
  /// Runs `apply` with `call` as the call its entries and events belong to, then restores the
  /// active one: a standby summary commits beside a conversation call that is still running.
  pub(in crate::session) fn with_model_call<R>(
    &mut self,
    call: ModelCallId,
    apply: impl FnOnce(&mut Self) -> Result<R, SessionError>,
  ) -> Result<R, SessionError> {
    let active = self.record.active_model_call.replace(call);
    let result = apply(self);
    self.record.active_model_call = active;
    result
  }
  /// Creates a list of the session's own, named `{key}/lists/{n}` by the next number.
  pub(in crate::session) fn allocate_list<T: StoredValue>(
    &mut self,
  ) -> Result<ListId, SessionError> {
    let id = self.record.next_list_id;
    self.record.next_list_id = id.checked_add(1).ok_or(StorageError::InvalidRange)?;
    let list = ListId(format!("{}/lists/{id}", self.key));
    self.store.create_list::<T>(&list)?;
    Ok(list)
  }
  pub(in crate::session) fn load_entry(&mut self, id: EntryId) -> Result<Arc<Entry>, SessionError> {
    load_entry(self.store, &self.record.entries, id)
  }
  pub(in crate::session) fn load_event(
    &mut self,
    id: EventId,
  ) -> Result<Arc<SessionEvent>, SessionError> {
    load_event(self.store, &self.record.events, id)
  }
}

/// The entry `id` of the session whose entries are `entries`; `InvalidEntry` when there is none.
pub(in crate::session) fn load_entry(
  store: &mut Transaction<'_>,
  entries: &ListId,
  id: EntryId,
) -> Result<Arc<Entry>, SessionError> {
  store.get_item::<Entry>(entries, id.0 as u64)?.ok_or(SessionError::InvalidEntry(id))
}
/// The event `id` of the session whose events are `events`. History names only events that exist,
/// so a missing one is corruption.
pub(in crate::session) fn load_event(
  store: &mut Transaction<'_>,
  events: &ListId,
  id: EventId,
) -> Result<Arc<SessionEvent>, SessionError> {
  Ok(
    store
      .get_item::<SessionEvent>(events, id.0)?
      .ok_or_else(|| StorageError::Corrupt("missing history event".into()))?,
  )
}
