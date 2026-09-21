use crate::{
  executor::model::tokens::TokenMeasurement,
  protocol::Message,
  session::{
    CompactionReason, EntryId, EntryOrigin, Generation, GenerationId, GenerationStatus, Session,
    SessionError, SessionEvent,
  },
};

impl Session {
  /// Append a fresh active generation directly. The standby is not used or seeded by this path.
  pub(crate) fn commit_upstream_compaction(
    &mut self,
    generation: GenerationId,
    prefix: Vec<EntryId>,
    body: Vec<Message>,
    latest_user: Option<EntryId>,
    measurement: TokenMeasurement,
    reason: CompactionReason,
  ) -> Result<(), SessionError> {
    self.update(move |transaction| {
      let mut active = transaction.load_generation(transaction.record.active)?;
      if active.id != generation {
        return Err(SessionError::StaleGeneration);
      }
      let original_count = transaction.tx.list_len::<EntryId>(&active.entries)?;
      let list = transaction.create_list::<EntryId>()?;
      transaction.tx.append_items(&list, &prefix)?;
      for message in body {
        let entry = transaction.store_entry(message, EntryOrigin::Summary)?;
        transaction.tx.append_item(&list, &entry)?;
      }
      let compaction_cursor = transaction.tx.list_len::<EntryId>(&list)?;
      if let Some(entry) = latest_user {
        transaction.tx.append_item(&list, &entry)?;
      }
      let id = GenerationId(
        transaction.tx.list_len::<Generation>(&transaction.record.generations)? as usize,
      );
      transaction.tx.append_item(
        &transaction.record.generations,
        &Generation {
          id,
          status: GenerationStatus::Active,
          entries: list,
          config: transaction.record.config.clone(),
          source: None,
          compaction_cursor,
        },
      )?;
      active.status = GenerationStatus::Sealed;
      transaction.save_generation(&active)?;
      transaction.record.active = id;
      // A session may previously have used local summaries. Keep its existing standby handle,
      // but discard those context references so a later local run starts from the new generation.
      let mut standby = transaction.load_generation(transaction.record.standby)?;
      if standby.source.is_some() || transaction.tx.list_len::<EntryId>(&standby.entries)? != 0 {
        standby.entries = transaction.create_list::<EntryId>()?;
        standby.source = None;
        standby.compaction_cursor = 0;
        transaction.save_generation(&standby)?;
      }
      transaction
        .record_event(SessionEvent::GenerationActivated { previous: active.id, active: id })?;
      transaction.record_event(SessionEvent::ContextCompacted {
        previous: active.id,
        active: id,
        reason,
        removed_entries: original_count - prefix.len() as u64 - u64::from(latest_user.is_some()),
        measurement,
      })
    })
  }
}
