use crate::{
  protocol::Message,
  session::{
    EntryId, EntryOrigin, Generation, GenerationId, GenerationStatus, HistoryItem, RunOutcome,
    Session, SessionError, SessionEvent,
  },
};

impl Session {
  /// Replace the encrypted compaction item with readable context in a fresh active generation.
  /// Existing entries after the item keep their IDs and order. Failure still creates a generation
  /// with an explicit placeholder so the new provider never receives an unreadable blob.
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
          compaction_cursor: prefix.len() as u64,
        },
      )?;
      active.status = GenerationStatus::Sealed;
      transaction.save_generation(&active)?;
      transaction.record.active = id;
      transaction.record_history_with_call(HistoryItem::Message(entry), None)?;
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
