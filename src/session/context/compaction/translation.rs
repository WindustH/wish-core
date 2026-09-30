use crate::{
  protocol::Message,
  session::{
    EntryId, EntryOrigin, GenerationId, GenerationStatus, RunOutcome, Session, SessionConfig,
    SessionError, SessionEvent,
  },
};

impl Session {
  /// Swap the encrypted compaction item for a copy that also carries its readable handoff, in a
  /// fresh active generation, and take `config`. Existing entries around the item keep their IDs
  /// and order. Failure still attaches an explicit placeholder so the new provider never receives
  /// an unreadable blob. Like a standby summary, the copy is context rather than conversation: it
  /// joins no history, and later summaries start after it.
  pub(crate) fn commit_compaction_translation(
    &mut self,
    expected_active: GenerationId,
    prefix: Vec<EntryId>,
    replacement: Message,
    suffix: Vec<EntryId>,
    failure: Option<RunOutcome>,
    config: SessionConfig,
  ) -> Result<(), SessionError> {
    config.validate()?;
    self.update(move |transaction| {
      if transaction.record.active != expected_active {
        return Err(SessionError::StaleGeneration);
      }
      // The generations below take the new config.
      transaction.record.config = config.clone();
      let list = transaction.allocate_list::<EntryId>()?;
      transaction.store.append_items(&list, &prefix)?;
      let entry = transaction.store_entry(replacement, EntryOrigin::Summary)?;
      transaction.store.append_item(&list, &entry)?;
      transaction.store.append_items(&list, &suffix)?;
      let cursor = prefix.len() as u64 + 1;
      let id = transaction.append_generation(GenerationStatus::Active, list, cursor, None)?;
      transaction.switch_active(id)?;
      transaction.reset_standby()?;
      let translated = failure.is_none();
      if let Some(outcome) = failure {
        transaction.record_event(SessionEvent::CompactionTranslationFailed { outcome })?;
      }
      transaction.record_event(SessionEvent::CompactionTranslationCompleted {
        previous: expected_active,
        active: id,
        entry,
        translated,
      })?;
      transaction.record_event(SessionEvent::ConfigUpdated(Box::new(config)))?;
      Ok(())
    })
  }
}
