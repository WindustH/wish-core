use super::context::validate_tool_pairs;
use super::persistence::{SessionRecord, SessionTransaction};
use super::statistics::{ModelCallRecord, Timestamp};
use super::{
  Entry, EntryId, EntryOrigin, Generation, GenerationId, GenerationStatus, HistoryRecord, Session,
  SessionConfig, SessionError, SessionEvent, SessionState,
};
use crate::protocol::Request;
use crate::storage::{ListId, OwnerGuard, Storage, StorageOptions};
use serde_json::Value;

impl Session {
  /// Convenience constructor using the same SQLite implementation in memory.
  pub fn new(config: SessionConfig) -> Result<Self, SessionError> {
    Self::create(Storage::open_in_memory(StorageOptions::default())?, "default", config)
  }
  pub fn create(storage: Storage, id: &str, config: SessionConfig) -> Result<Self, SessionError> {
    if let Some(compaction) = &config.compaction {
      compaction.validate()?;
    }
    let key = Self::build_key(id);
    let owner = storage.claim_owner(&key)?;
    let target = key.clone();
    let recorded_at = Timestamp::now();
    let record = storage.transaction(move |tx| -> Result<_, SessionError> {
      let key = target;
      let mut record = SessionRecord {
        metadata: Value::Null,
        config: config.clone(),
        state: SessionState::Idle,
        active: GenerationId(0),
        standby: GenerationId(1),
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
      tx.create_list::<ModelCallRecord>(&record.model_calls)?;
      tx.create_list::<Entry>(&record.entries)?;
      tx.create_list::<SessionEvent>(&record.events)?;
      tx.create_list::<HistoryRecord>(&record.history)?;
      tx.create_list::<Generation>(&record.generations)?;
      tx.create_list::<EntryId>(&record.queue)?;
      for (id, status) in
        [(GenerationId(0), GenerationStatus::Active), (GenerationId(1), GenerationStatus::Standby)]
      {
        let entries = ListId(format!("{key}/generation/{}/0", id.0));
        tx.create_list::<EntryId>(&entries)?;
        tx.append_item(
          &record.generations,
          &Generation {
            id,
            status,
            entries,
            config: config.clone(),
            source: None,
            compaction_cursor: 0,
          },
        )?;
      }
      let mut transaction = SessionTransaction { record: &mut record, tx, key: &key, recorded_at };
      transaction.record_event(SessionEvent::Created(Box::new(config)))?;
      tx.create_object(&key, &record)?;
      Ok(record)
    })?;
    Ok(Self::from_record(storage, key, owner, record))
  }
  pub fn load(storage: Storage, id: &str) -> Result<Self, SessionError> {
    let key = Self::build_key(id);
    let owner = storage.claim_owner(&key)?;
    let target = key.clone();
    let record = storage.transaction(move |tx| -> Result<_, SessionError> {
      let record = (*tx.load_object::<SessionRecord>(&target)?).clone();
      Ok(record)
    })?;
    Ok(Self::from_record(storage, key, owner, record))
  }
  pub(in crate::session) fn build_key(id: &str) -> String {
    format!("session/{}", hex::encode(id.as_bytes()))
  }
  pub(in crate::session) fn from_record(
    storage: Storage,
    key: String,
    owner: OwnerGuard,
    record: SessionRecord,
  ) -> Self {
    Self { storage, key, _owner: owner, record, control: Default::default() }
  }
  /// Import request content and settings; session tool selection is always Auto.
  pub fn from_request(request: Request) -> Result<Self, SessionError> {
    validate_tool_pairs(request.conversation.iter())?;
    let config = SessionConfig {
      model: request.model,
      stream: request.stream,
      tools: request.tools,
      max_output_tokens: request.max_output_tokens,
      reasoning: request.reasoning,
      cache: request.cache,
      run: Default::default(),
      compaction: None,
    };
    let mut session = Self::new(config)?;
    session.update(move |transaction| {
      for message in request.conversation {
        transaction.append_message(message, EntryOrigin::Imported)?;
      }
      Ok(())
    })?;
    Ok(session)
  }
}

impl Session {
  /// Permanently delete an inactive session and all its stored history and generations.
  pub fn delete(&mut self) -> Result<(), SessionError> {
    self.require_stable()?;
    let key = self.key.clone();
    self.storage.transaction(move |tx| tx.delete_namespace(&key))?;
    Ok(())
  }
}

impl Session {
  /// Import a complete, protocol-valid conversation at an inactive boundary.
  pub fn import_history(
    &mut self,
    messages: Vec<crate::protocol::Message>,
  ) -> Result<(), SessionError> {
    self.require_stable()?;
    validate_tool_pairs(messages.iter())?;
    self.update(move |transaction| {
      for message in messages {
        transaction.append_message(message, EntryOrigin::Imported)?;
      }
      Ok(())
    })
  }
}
