//! Persistent sessions: small immutable entries, paged generations/history and transactional state.
mod config;
mod edit;
mod event;
mod generation;
mod generations;
mod history;
mod machine;
mod outcome;
mod queue;
pub(crate) mod state;

pub use config::{RunOptions, SessionConfig, ToolMode};
pub use event::SessionEvent;
pub use generation::{Generation, GenerationId, GenerationStatus};
pub use history::{Entry, EntryId, EntryOrigin, EventId, HistoryItem, HistoryRecord};
pub use outcome::RunOutcome;
pub use queue::SessionSender;
pub use state::{SessionPhase, SessionState, ToolExecution};

use crate::{
  protocol::{Message, Request},
  storage::{ListId, OwnerGuard, ReadList, Storage, StorageError, StorageOptions},
};
use edit::SessionEdit;
use queue::validate_input;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
  #[error(transparent)]
  Storage(#[from] StorageError),
  #[error("operation requires a stable session boundary")]
  Busy,
  #[error("only user, system and developer messages can be enqueued")]
  InvalidInput,
  #[error("invalid entry reference: {0:?}")]
  InvalidEntry(EntryId),
  #[error("queued input cannot be used before consumption: {0:?}")]
  PendingInput(EntryId),
  #[error("context contains an unpaired or mismatched tool call/result")]
  UnpairedTools,
  #[error("standby was not prepared against the active generation")]
  StaleGeneration,
  #[error("session must be explicitly resumed before running")]
  Suspended,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SessionRecord {
  metadata: Value,
  config: SessionConfig,
  state: SessionState,
  active: GenerationId,
  standby: GenerationId,
  entries: ListId,
  events: ListId,
  history: ListId,
  generations: ListId,
  queue: ListId,
  queue_head: u64,
  #[serde(default)]
  next_list_id: u64,
}

/// A single running owner backed by transactional storage. Other tasks use SessionSender.
pub struct Session {
  storage: Storage,
  key: String,
  _owner: OwnerGuard,
  record: SessionRecord,
}
impl Session {
  /// Convenience constructor using the same SQLite implementation in memory.
  pub fn new(config: SessionConfig) -> Result<Self, SessionError> {
    Self::create(Storage::open_in_memory(StorageOptions::default())?, "default", config)
  }
  pub fn create(storage: Storage, id: &str, config: SessionConfig) -> Result<Self, SessionError> {
    let key = Self::build_key(id);
    let owner = storage.claim_owner(&key)?;
    let target = key.clone();
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
      };
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
          &Generation { id, status, entries, config: config.clone(), source: None },
        )?;
      }
      let mut edit = SessionEdit { record: &mut record, tx, key: &key };
      edit.record_event(SessionEvent::Created(Box::new(config)))?;
      tx.create_object(&key, &record)?;
      Ok(record)
    })?;
    Ok(Self::from_record(storage, key, owner, record))
  }
  pub fn load(storage: Storage, id: &str) -> Result<Self, SessionError> {
    let key = Self::build_key(id);
    let owner = storage.claim_owner(&key)?;
    let stored = storage.open_object::<SessionRecord>(&key).load()?;
    Ok(Self::from_record(storage, key, owner, (*stored).clone()))
  }
  fn build_key(id: &str) -> String {
    format!("session/{}", hex::encode(id.as_bytes()))
  }
  fn from_record(storage: Storage, key: String, owner: OwnerGuard, record: SessionRecord) -> Self {
    Self { storage, key, _owner: owner, record }
  }
  pub fn from_request(request: Request) -> Result<Self, SessionError> {
    edit::validate_tool_pairs(request.conversation.iter())?;
    let config = SessionConfig {
      model: request.model,
      stream: request.stream,
      tools: request.tools,
      tool_choice: request.tool_choice,
      max_output_tokens: request.max_output_tokens,
      reasoning: request.reasoning,
      cache: request.cache,
      run: Default::default(),
    };
    let mut session = Self::new(config)?;
    session.update(move |edit| {
      for message in request.conversation {
        edit.append_message(message, EntryOrigin::Imported)?;
      }
      Ok(())
    })?;
    Ok(session)
  }
  fn update<R: Send + 'static>(
    &mut self,
    apply: impl FnOnce(&mut SessionEdit<'_, '_>) -> Result<R, SessionError> + Send + 'static,
  ) -> Result<R, SessionError> {
    let mut record = self.record.clone();
    let key = self.key.clone();
    let (result, record) = self.storage.transaction(move |tx| -> Result<_, SessionError> {
      let result = apply(&mut SessionEdit { record: &mut record, tx, key: &key })?;
      tx.save_object(&key, &record)?;
      Ok((result, record))
    })?;
    self.record = record;
    Ok(result)
  }
  pub fn get_metadata(&self) -> &Value {
    &self.record.metadata
  }
  pub fn set_metadata(&mut self, value: Value) -> Result<(), SessionError> {
    self.update(move |edit| {
      edit.record.metadata = value.clone();
      edit.record_event(SessionEvent::MetadataUpdated(value))
    })
  }
  pub fn get_config(&self) -> &SessionConfig {
    &self.record.config
  }
  pub fn set_config(&mut self, config: SessionConfig) -> Result<(), SessionError> {
    self.require_stable()?;
    self.update(move |edit| {
      for id in [edit.record.active, edit.record.standby] {
        let mut generation = edit.load_generation(id)?;
        generation.config = config.clone();
        edit.save_generation(&generation)?;
      }
      edit.record.config = config.clone();
      edit.record_event(SessionEvent::ConfigUpdated(Box::new(config)))
    })
  }
  pub fn get_state(&self) -> &SessionState {
    &self.record.state
  }
  pub fn get_history(&self) -> ReadList<HistoryRecord> {
    self.storage.open_list(&self.record.history).read_only()
  }
  pub fn get_entries(&self) -> ReadList<Entry> {
    self.storage.open_list(&self.record.entries).read_only()
  }
  pub fn get_events(&self) -> ReadList<SessionEvent> {
    self.storage.open_list(&self.record.events).read_only()
  }
  pub fn get_entry(&self, id: EntryId) -> Result<Option<Arc<Entry>>, SessionError> {
    Ok(self.get_entries().get(id.0 as u64)?)
  }
  pub fn get_event(&self, id: EventId) -> Result<Option<Arc<SessionEvent>>, SessionError> {
    Ok(self.get_events().get(id.0)?)
  }
  pub fn get_generations(&self) -> ReadList<Generation> {
    self.storage.open_list(&self.record.generations).read_only()
  }
  pub fn get_active_generation(&self) -> Result<Arc<Generation>, SessionError> {
    self.load_generation(self.record.active)
  }
  pub fn get_standby_generation(&self) -> Result<Arc<Generation>, SessionError> {
    self.load_generation(self.record.standby)
  }
  fn load_generation(&self, id: GenerationId) -> Result<Arc<Generation>, SessionError> {
    self
      .get_generations()
      .get(id.0 as u64)?
      .ok_or_else(|| StorageError::Corrupt("missing generation".into()).into())
  }
  pub fn get_generation_entries(
    &self,
    id: GenerationId,
  ) -> Result<ReadList<EntryId>, SessionError> {
    Ok(self.storage.open_list(&self.load_generation(id)?.entries).read_only())
  }
  /// The queue log is paged too. Positions below get_queue_head() have already been consumed.
  pub fn get_message_queue(&self) -> ReadList<EntryId> {
    self.storage.open_list(&self.record.queue).read_only()
  }
  pub fn get_queue_head(&self) -> u64 {
    self.record.queue_head
  }
  pub fn create_sender(&self) -> SessionSender {
    SessionSender { storage: self.storage.clone(), key: self.key.clone() }
  }
  pub fn enqueue_message(&mut self, message: Message) -> Result<EntryId, SessionError> {
    validate_input(&message)?;
    self.update(move |edit| edit.enqueue_message(message))
  }
  /// Notice durable queued input at a stable boundary. Already-running phases are unchanged.
  pub fn collect_inputs(&mut self) -> Result<(), SessionError> {
    if !matches!(self.record.state, SessionState::Idle) {
      return Ok(());
    }
    if self.get_message_queue().len()? == self.record.queue_head {
      return Ok(());
    }
    self.update(move |edit| {
      if edit.record.queue_head < edit.tx.list_len::<EntryId>(&edit.record.queue)? {
        edit.transition_to(SessionState::Ready { completed_turns: 0, needs_model: true })?;
      }
      Ok(())
    })
  }
  pub fn resume(&mut self) -> Result<(), SessionError> {
    self.require_stable()?;
    self.update(move |edit| {
      edit.transition_to(SessionState::Ready { completed_turns: 0, needs_model: true })
    })
  }
  pub fn build_request(&self) -> Result<Request, SessionError> {
    let mut record = self.record.clone();
    let key = self.key.clone();
    self
      .storage
      .transaction(move |tx| SessionEdit { record: &mut record, tx, key: &key }.build_request())
  }
  pub fn create_context_entry(&mut self, message: Message) -> Result<EntryId, SessionError> {
    self.update(move |edit| {
      let entry = edit.store_entry(message, EntryOrigin::Context)?;
      edit.record_event(SessionEvent::ContextEntryCreated { entry })?;
      Ok(entry)
    })
  }
  pub(crate) fn require_stable(&self) -> Result<(), SessionError> {
    if self.record.state.is_stable() { Ok(()) } else { Err(SessionError::Busy) }
  }
  pub(crate) fn record_event(&mut self, event: SessionEvent) -> Result<(), SessionError> {
    let mut record = self.record.clone();
    let key = self.key.clone();
    self
      .storage
      .transaction(move |tx| SessionEdit { record: &mut record, tx, key: &key }.record_event(event))
  }
}
