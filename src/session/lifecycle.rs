use super::context::validate_tool_pairs;
use super::persistence::{SessionRecord, SessionTransaction};
use super::statistics::ModelCallRecord;
use super::{
  Entry, EntryId, EntryOrigin, Generation, GenerationId, GenerationStatus, HistoryRecord, Session,
  SessionConfig, SessionError, SessionEvent, SessionReader, SessionState,
};
use crate::protocol::Message;
use crate::storage::{ListId, OwnerGuard, Storage, StorageOptions};
use crate::utils::time::Timestamp;
use serde_json::Value;

impl Session {
  /// Convenience constructor using the same SQLite implementation in memory.
  pub fn new(config: SessionConfig) -> Result<Self, SessionError> {
    Self::create(Storage::open_in_memory(StorageOptions::default())?, "default", config)
  }
  pub fn create(storage: Storage, id: &str, config: SessionConfig) -> Result<Self, SessionError> {
    config.validate()?;
    let key = Self::build_key(id);
    let owner = storage.claim_owner(&key)?;
    let target = key.clone();
    let recorded_at = Timestamp::now();
    let record = storage.transaction(move |store| -> Result<_, SessionError> {
      let key = target;
      let mut record = SessionRecord {
        metadata: Value::Null,
        config: config.clone(),
        state: SessionState::Idle,
        // Both are set below, as the generations are appended.
        active: GenerationId(0),
        standby: GenerationId(0),
        entries: ListId(format!("{key}/entries")),
        events: ListId(format!("{key}/events")),
        history: ListId(format!("{key}/history")),
        generations: ListId(format!("{key}/generations")),
        queue: ListId(format!("{key}/queue")),
        queue_head: 0,
        next_list_id: 0,
        model_calls: ListId(format!("{key}/model_calls")),
        active_model_call: None,
      };
      store.create_list::<ModelCallRecord>(&record.model_calls)?;
      store.create_list::<Entry>(&record.entries)?;
      store.create_list::<SessionEvent>(&record.events)?;
      store.create_list::<HistoryRecord>(&record.history)?;
      store.create_list::<Generation>(&record.generations)?;
      store.create_list::<EntryId>(&record.queue)?;
      let mut transaction =
        SessionTransaction { record: &mut record, store, key: &key, recorded_at };
      let entries = transaction.allocate_list::<EntryId>()?;
      transaction.record.active =
        transaction.append_generation(GenerationStatus::Active, entries, 0, None)?;
      transaction.append_standby(&[], 0, None)?;
      transaction.record_event(SessionEvent::Created(Box::new(config)))?;
      store.create_object(&key, &record)?;
      Ok(record)
    })?;
    Ok(Self::from_record(storage, key, owner, record))
  }
  pub fn load(storage: Storage, id: &str) -> Result<Self, SessionError> {
    let key = Self::build_key(id);
    let owner = storage.claim_owner(&key)?;
    let target = key.clone();
    let record = storage.transaction(move |store| -> Result<_, SessionError> {
      Ok((*store.load_object::<SessionRecord>(&target)?).clone())
    })?;
    Ok(Self::from_record(storage, key, owner, record))
  }
  fn build_key(id: &str) -> String {
    format!("session/{}", hex::encode(id.as_bytes()))
  }
  fn from_record(storage: Storage, key: String, owner: OwnerGuard, record: SessionRecord) -> Self {
    Self {
      reader: SessionReader::new(storage.clone(), &record),
      storage,
      key,
      _owner: owner,
      record,
      arrivals: Default::default(),
    }
  }
}

impl Session {
  /// Permanently delete an inactive session and all its stored history and generations.
  pub fn delete(&mut self) -> Result<(), SessionError> {
    self.require_stable()?;
    let key = self.key.clone();
    self.storage.transaction(move |store| store.delete_namespace(&key))?;
    Ok(())
  }
}

impl Session {
  /// Import a complete, protocol-valid conversation at an inactive boundary. Messages join the
  /// active context and its history as `Imported`; those that were summaries or other context-only
  /// entries where they came from (`Summary`, `Context`) keep that origin and join the context but
  /// no history, as they did there, and later summaries start after them.
  pub fn import_history(
    &mut self,
    messages: Vec<(Message, EntryOrigin)>,
  ) -> Result<(), SessionError> {
    self.require_stable()?;
    validate_tool_pairs(messages.iter().map(|(message, _)| message))?;
    self.update(move |transaction| {
      let mut generation = transaction.load_generation(transaction.record.active)?;
      let start = transaction.store.list_len::<EntryId>(&generation.entries)?;
      let mut cursor = None;
      for (position, (message, origin)) in messages.into_iter().enumerate() {
        if matches!(origin, EntryOrigin::Summary | EntryOrigin::Context) {
          let entry = transaction.store_entry(message, origin)?;
          transaction.store.append_item(&generation.entries, &entry)?;
          cursor = Some(start + position as u64 + 1);
        } else {
          transaction.append_message(message, EntryOrigin::Imported)?;
        }
      }
      if let Some(cursor) = cursor {
        generation.compaction_cursor = generation.compaction_cursor.max(cursor);
        transaction.save_generation(&generation)?;
      }
      Ok(())
    })
  }
}
