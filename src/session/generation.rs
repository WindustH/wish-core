use super::edit::{SessionEdit, ToolPairValidator};
use super::{Entry, EntryId, EntryOrigin, Session, SessionConfig, SessionError, SessionEvent};
use crate::storage::{ListId, PAGE_SIZE};

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
    self.update(move |edit| edit.prepare_generation(entries))
  }
  pub fn activate_standby_generation(&mut self) -> Result<(), SessionError> {
    self.require_stable()?;
    self.update(move |edit| edit.activate_generation())
  }
}
impl SessionEdit<'_, '_> {
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
    for id in entries {
      let entry = self
        .tx
        .get_item::<Entry>(&self.record.entries, id.0 as u64)?
        .ok_or(SessionError::InvalidEntry(id))?;
      if entry.origin == EntryOrigin::Input && pending.as_ref().is_some_and(|first| id.0 >= first.0)
      {
        return Err(SessionError::PendingInput(id));
      }
      validator.accept(&entry.message)?;
      self.tx.append_item(&list, &id)?;
    }
    validator.finish()?;
    let entry_count = self.tx.list_len::<EntryId>(&list)?;
    standby.entries = list.clone();
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
      },
    )?;
    self.record.standby = id;
    self.record_event(SessionEvent::GenerationActivated { previous: active.id, active: standby.id })
  }
}
