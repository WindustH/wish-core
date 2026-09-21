use super::validation::ToolPairValidator;
use crate::session::persistence::SessionTransaction;
use crate::session::{
  Entry, EntryId, EntryOrigin, Session, SessionConfig, SessionError, SessionEvent,
};
use crate::storage::{ListId, PAGE_SIZE, ReadList, StorageError};
use std::sync::Arc;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GenerationId(pub usize);

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerationStatus {
  Active,
  Standby,
  Sealed,
}

/// Ordered direct entry references; never references another generation's content.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Generation {
  pub id: GenerationId,
  pub status: GenerationStatus,
  pub entries: ListId,
  pub config: SessionConfig,
  /// First entry still eligible for summarization; prior summaries are never summarized again.
  #[serde(default)]
  pub compaction_cursor: u64,
  /// The stable source prefix captured when standby was prepared.
  pub(crate) source: Option<(GenerationId, u64)>,
}

impl Session {
  /// Replacement entry IDs are consumed incrementally inside a transaction.
  pub fn prepare_standby_generation(
    &mut self,
    entries: impl IntoIterator<Item = EntryId> + Send + 'static,
  ) -> Result<(), SessionError> {
    self.require_stable()?;
    self.update(move |transaction| transaction.prepare_generation(entries))
  }
  pub fn activate_standby_generation(&mut self) -> Result<(), SessionError> {
    self.require_stable()?;
    self.update(move |transaction| transaction.activate_generation())
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
  pub(in crate::session) fn load_generation(
    &self,
    id: GenerationId,
  ) -> Result<Arc<Generation>, SessionError> {
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
}
impl SessionTransaction<'_, '_> {
  fn prepare_generation(
    &mut self,
    entries: impl IntoIterator<Item = EntryId>,
  ) -> Result<(), SessionError> {
    let active = self.load_generation(self.record.active)?;
    let source_length = self.tx.list_len::<EntryId>(&active.entries)?;
    let mut standby = self.load_generation(self.record.standby)?;
    let list = self.create_list::<EntryId>()?;
    // Input entries and queue positions are allocated together, in ascending entry-ID order.
    // The first unconsumed ID is enough to classify input entries; do not load the whole queue.
    let pending = self.tx.get_item::<EntryId>(&self.record.queue, self.record.queue_head)?;
    let mut validator = ToolPairValidator::default();
    let mut compaction_cursor = 0;
    for (position, id) in entries.into_iter().enumerate() {
      let entry = self
        .tx
        .get_item::<Entry>(&self.record.entries, id.0 as u64)?
        .ok_or(SessionError::InvalidEntry(id))?;
      if entry.origin == EntryOrigin::Input && pending.as_ref().is_some_and(|first| id.0 >= first.0)
      {
        return Err(SessionError::PendingInput(id));
      }
      validator.accept(&entry.message)?;
      if entry.origin == EntryOrigin::Summary
        || matches!(entry.message, crate::protocol::Message::UpstreamCompaction { .. })
      {
        compaction_cursor = position as u64 + 1;
      }
      self.tx.append_item(&list, &id)?;
    }
    validator.finish()?;
    let entry_count = self.tx.list_len::<EntryId>(&list)?;
    standby.entries = list.clone();
    standby.compaction_cursor = compaction_cursor;
    standby.source = Some((active.id, source_length));
    self.save_generation(&standby)?;
    self.record_event(SessionEvent::GenerationPrepared {
      generation: standby.id,
      entries: list,
      entry_count,
      source_generation: active.id,
      source_length,
    })
  }
  fn activate_generation(&mut self) -> Result<(), SessionError> {
    let mut active = self.load_generation(self.record.active)?;
    let mut standby = self.load_generation(self.record.standby)?;
    let Some((source, source_length)) = standby.source else {
      return Err(SessionError::StaleGeneration);
    };
    let length = self.tx.list_len::<EntryId>(&active.entries)?;
    if source != active.id || source_length > length {
      return Err(SessionError::StaleGeneration);
    }
    // Active is append-only. Validate the candidate and its newer tail without loading the prefix.
    let mut validator = ToolPairValidator::default();
    for (list, start, end, copy) in [
      (&standby.entries, 0, self.tx.list_len::<EntryId>(&standby.entries)?, false),
      (&active.entries, source_length, length, true),
    ] {
      let mut position = start;
      while position < end {
        let limit = (end - position).min(PAGE_SIZE) as usize;
        let page = self.tx.read_page::<EntryId>(list, position, limit)?;
        for id in &page.items {
          let entry = self
            .tx
            .get_item::<Entry>(&self.record.entries, id.0 as u64)?
            .ok_or(SessionError::InvalidEntry(**id))?;
          validator.accept(&entry.message)?;
          if copy {
            self.tx.append_item(&standby.entries, id.as_ref())?;
          }
        }
        position += page.items.len() as u64;
      }
    }
    validator.finish()?;
    active.status = GenerationStatus::Sealed;
    standby.status = GenerationStatus::Active;
    standby.source = None;
    self.save_generation(&active)?;
    self.save_generation(&standby)?;
    self.record.active = standby.id;
    let id = GenerationId(self.tx.list_len::<Generation>(&self.record.generations)? as usize);
    let entries = ListId(format!("{}/generation/{}/0", self.key, id.0));
    self.tx.create_list::<EntryId>(&entries)?;
    self.tx.append_item(
      &self.record.generations,
      &Generation {
        id,
        status: GenerationStatus::Standby,
        entries,
        config: self.record.config.clone(),
        source: None,
        compaction_cursor: 0,
      },
    )?;
    self.record.standby = id;
    self.record_event(SessionEvent::GenerationActivated { previous: active.id, active: standby.id })
  }
  pub fn load_generation(&mut self, id: GenerationId) -> Result<Generation, SessionError> {
    self
      .tx
      .get_item::<Generation>(&self.record.generations, id.0 as u64)?
      .map(|item| (*item).clone())
      .ok_or_else(|| StorageError::Corrupt("missing generation".into()).into())
  }
  pub fn save_generation(&mut self, generation: &Generation) -> Result<(), SessionError> {
    Ok(self.tx.set_item(&self.record.generations, generation.id.0 as u64, generation)?)
  }
}
