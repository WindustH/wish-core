//! Session record layout and transaction boundaries; database/cache mechanics live in storage.
use super::statistics::{ModelCallId, Timestamp};
use super::{GenerationId, Session, SessionConfig, SessionError, SessionState};
use crate::storage::{ListId, StorageError, Transaction};
use serde_json::Value;

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

pub(super) struct SessionTransaction<'a, 'db> {
  pub record: &'a mut SessionRecord,
  pub tx: &'a mut Transaction<'db>,
  pub key: &'a str,
  pub recorded_at: Timestamp,
}
impl Session {
  pub(in crate::session) fn update<R: Send + 'static>(
    &mut self,
    apply: impl FnOnce(&mut SessionTransaction<'_, '_>) -> Result<R, SessionError> + Send + 'static,
  ) -> Result<R, SessionError> {
    let mut record = self.record.clone();
    let key = self.key.clone();
    let recorded_at = Timestamp::now();
    let (result, record) = self.storage.transaction(move |tx| -> Result<_, SessionError> {
      let result =
        apply(&mut SessionTransaction { record: &mut record, tx, key: &key, recorded_at })?;
      tx.save_object(&key, &record)?;
      Ok((result, record))
    })?;
    self.record = record;
    Ok(result)
  }
}
impl SessionTransaction<'_, '_> {
  pub fn create_list<T: crate::storage::StoredValue>(&mut self) -> Result<ListId, SessionError> {
    let id = self.record.next_list_id;
    self.record.next_list_id = id.checked_add(1).ok_or(StorageError::InvalidRange)?;
    let list = ListId(format!("{}/lists/{id}", self.key));
    self.tx.create_list::<T>(&list)?;
    Ok(list)
  }
}
