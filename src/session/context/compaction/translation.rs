use crate::{
  protocol::Message,
  session::{
    EntryId, EntryOrigin, Generation, GenerationId, GenerationStatus, RunOutcome, Session,
    SessionError, SessionEvent,
  },
};

impl Session {
  /// Swap the encrypted compaction item for a copy that also carries its readable handoff, in a
  /// fresh active generation. Existing entries around the item keep their IDs and order. Failure
  /// still attaches an explicit placeholder so the new provider never receives an unreadable blob.
  /// Like a standby summary, the copy is context rather than conversation: it joins no history,
  /// and later summaries start after it.
  pub(crate) fn commit_compaction_translation(
    &mut self,
    source: GenerationId,
    prefix: Vec<EntryId>,
    replacement: Message,
    suffix: Vec<EntryId>,
    failure: Option<RunOutcome>,
    config: crate::session::SessionConfig,
  ) -> Result<(), SessionError> {
    if let Some(compaction) = &config.compaction {
      compaction.validate()?;
    }
    self.update(move |transaction| {
      let mut active = transaction.load_generation(transaction.record.active)?;
      if active.id != source {
        return Err(SessionError::StaleGeneration);
      }
      let list = transaction.create_list::<EntryId>()?;
      transaction.tx.append_items(&list, &prefix)?;
      let entry = transaction.store_entry(replacement, EntryOrigin::Summary)?;
      transaction.tx.append_item(&list, &entry)?;
      transaction.tx.append_items(&list, &suffix)?;
      let id = GenerationId(
        transaction.tx.list_len::<Generation>(&transaction.record.generations)? as usize,
      );
      transaction.tx.append_item(
        &transaction.record.generations,
        &Generation {
          id,
          status: GenerationStatus::Active,
          entries: list,
          config: config.clone(),
          source: None,
          compaction_cursor: prefix.len() as u64 + 1,
        },
      )?;
      active.status = GenerationStatus::Sealed;
      transaction.save_generation(&active)?;
      transaction.record.active = id;
      let mut standby = transaction.load_generation(transaction.record.standby)?;
      standby.entries = transaction.create_list::<EntryId>()?;
      standby.source = None;
      standby.compaction_cursor = 0;
      standby.config = config.clone();
      transaction.save_generation(&standby)?;
      transaction.record.config = config.clone();
      transaction
        .record_event(SessionEvent::GenerationActivated { previous: source, active: id })?;
      let translated = failure.is_none();
      if let Some(outcome) = failure {
        transaction.record_event(SessionEvent::CompactionTranslationFailed { outcome })?;
      }
      transaction.record_event(SessionEvent::CompactionTranslationCompleted {
        previous: source,
        active: id,
        entry,
        translated,
      })?;
      transaction.record_event(SessionEvent::ConfigUpdated(Box::new(config)))
    })
  }
}
