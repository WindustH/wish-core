use super::validation::ToolPairValidator;
use crate::session::persistence::SessionTransaction;
use crate::session::{EntryId, EntryOrigin, Session, SessionConfig, SessionError, SessionEvent};
use crate::storage::{ListId, StorageError};

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
  pub compaction_cursor: u64,
  /// The stable source prefix captured when standby was prepared.
  pub(crate) source: Option<(GenerationId, u64)>,
}
impl Generation {
  /// How far into `active` this standby replaces it - to the end of the last span summarized, or
  /// to the length `active` had when the standby was prepared - when it was prepared from `active`.
  /// A standby summary starts there.
  pub(crate) fn summarized_end(&self, active: &Generation) -> Option<u64> {
    self.source.filter(|(id, _)| *id == active.id).map(|(_, end)| end)
  }
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
}
impl SessionTransaction<'_, '_> {
  fn prepare_generation(
    &mut self,
    entries: impl IntoIterator<Item = EntryId>,
  ) -> Result<(), SessionError> {
    let active = self.load_generation(self.record.active)?;
    let source_length = self.store.list_len::<EntryId>(&active.entries)?;
    let mut standby = self.load_generation(self.record.standby)?;
    self.replace_entries(&mut standby)?;
    // Input entries and queue positions are allocated together, in ascending entry-ID order.
    // The first unconsumed ID is enough to classify input entries; do not load the whole queue.
    let pending = self.store.get_item::<EntryId>(&self.record.queue, self.record.queue_head)?;
    let mut validator = ToolPairValidator::default();
    let mut compaction_cursor = 0;
    for (position, id) in entries.into_iter().enumerate() {
      let entry = self.load_entry(id)?;
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
      self.store.append_item(&standby.entries, &id)?;
    }
    validator.finish()?;
    let entry_count = self.store.list_len::<EntryId>(&standby.entries)?;
    standby.compaction_cursor = compaction_cursor;
    standby.source = Some((active.id, source_length));
    self.save_generation(&standby)?;
    self.record_event(SessionEvent::GenerationPrepared {
      generation: standby.id,
      entries: standby.entries,
      entry_count,
      source_generation: active.id,
      source_length,
    })?;
    Ok(())
  }
  fn activate_generation(&mut self) -> Result<(), SessionError> {
    let active = self.load_generation(self.record.active)?;
    let mut standby = self.load_generation(self.record.standby)?;
    let Some((source, source_length)) = standby.source else {
      return Err(SessionError::StaleGeneration);
    };
    let length = self.store.list_len::<EntryId>(&active.entries)?;
    if source != active.id || source_length > length {
      return Err(SessionError::StaleGeneration);
    }
    // Active is append-only. Validate the candidate and its newer tail without loading the prefix.
    let mut validator = ToolPairValidator::default();
    for id in self.store.read_from::<EntryId>(&standby.entries, 0)? {
      validator.accept(&self.load_entry(*id)?.message)?;
    }
    for id in self.store.read_from::<EntryId>(&active.entries, source_length)? {
      validator.accept(&self.load_entry(*id)?.message)?;
      self.store.append_item(&standby.entries, id.as_ref())?;
    }
    validator.finish()?;
    standby.source = None;
    self.save_generation(&standby)?;
    self.promote_standby()?;
    self.append_standby(&[], 0, None)
  }
  /// The generation `id`, to edit and save.
  pub(in crate::session) fn load_generation(
    &mut self,
    id: GenerationId,
  ) -> Result<Generation, SessionError> {
    Ok(
      self
        .store
        .get_item::<Generation>(&self.record.generations, id.0 as u64)?
        .map(|item| (*item).clone())
        .ok_or_else(|| StorageError::Corrupt("missing generation".into()))?,
    )
  }
  pub(in crate::session) fn save_generation(
    &mut self,
    generation: &Generation,
  ) -> Result<(), SessionError> {
    Ok(self.store.set_item(&self.record.generations, generation.id.0 as u64, generation)?)
  }
  /// Appends a generation over `entries`, with the session's config, and returns its id.
  pub(in crate::session) fn append_generation(
    &mut self,
    status: GenerationStatus,
    entries: ListId,
    compaction_cursor: u64,
    source: Option<(GenerationId, u64)>,
  ) -> Result<GenerationId, SessionError> {
    let id = GenerationId(self.store.list_len::<Generation>(&self.record.generations)? as usize);
    let config = self.record.config.clone();
    let generation = Generation { id, status, entries, config, compaction_cursor, source };
    self.store.append_item(&self.record.generations, &generation)?;
    Ok(id)
  }
  /// Makes `next` the active generation, sealing the one it replaces, and records the switch.
  /// Returns the id of the generation replaced.
  pub(in crate::session) fn switch_active(
    &mut self,
    next: GenerationId,
  ) -> Result<GenerationId, SessionError> {
    let mut previous = self.load_generation(self.record.active)?;
    previous.status = GenerationStatus::Sealed;
    self.save_generation(&previous)?;
    let mut activated = self.load_generation(next)?;
    activated.status = GenerationStatus::Active;
    activated.source = None;
    self.save_generation(&activated)?;
    self.record.active = next;
    self.record_event(SessionEvent::GenerationActivated { previous: previous.id, active: next })?;
    Ok(previous.id)
  }
  /// Makes the standby the active generation; see [`SessionTransaction::switch_active`]. The
  /// session has no standby until [`SessionTransaction::append_standby`] gives it a new one.
  pub(in crate::session) fn promote_standby(&mut self) -> Result<GenerationId, SessionError> {
    self.switch_active(self.record.standby)
  }
  /// Appends a new standby generation holding `seed`, and makes it the session's standby.
  pub(in crate::session) fn append_standby(
    &mut self,
    seed: &[EntryId],
    compaction_cursor: u64,
    source: Option<(GenerationId, u64)>,
  ) -> Result<(), SessionError> {
    let entries = self.allocate_list::<EntryId>()?;
    self.store.append_items(&entries, seed)?;
    self.record.standby =
      self.append_generation(GenerationStatus::Standby, entries, compaction_cursor, source)?;
    Ok(())
  }
  /// Empties the standby: no entries, nothing summarized, prepared from nothing, with the session's
  /// config.
  pub(in crate::session) fn reset_standby(&mut self) -> Result<(), SessionError> {
    let mut standby = self.load_generation(self.record.standby)?;
    self.replace_entries(&mut standby)?;
    standby.source = None;
    standby.compaction_cursor = 0;
    standby.config = self.record.config.clone();
    self.save_generation(&standby)
  }
  /// Gives the standby `generation` a new, empty list for its entries, deleting the one it had.
  /// Only a standby's list is replaced, and nothing reads it once its generation names another:
  /// active and sealed generations keep theirs.
  pub(in crate::session) fn replace_entries(
    &mut self,
    generation: &mut Generation,
  ) -> Result<(), SessionError> {
    let replaced = std::mem::replace(&mut generation.entries, self.allocate_list::<EntryId>()?);
    self.store.delete_list::<EntryId>(&replaced)?;
    Ok(())
  }
}
