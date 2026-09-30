use super::TokenMeasurement;
use crate::{
  protocol::Message,
  session::{
    CompactionReason, EntryId, EntryOrigin, GenerationId, GenerationStatus, Session, SessionError,
    SessionEvent,
  },
};

impl Session {
  /// Append a fresh active generation directly. The standby is not used or seeded by this path.
  pub(crate) fn commit_upstream_compaction(
    &mut self,
    expected_active: GenerationId,
    prefix: Vec<EntryId>,
    body: Vec<Message>,
    latest_user: Option<EntryId>,
    measurement: TokenMeasurement,
    reason: CompactionReason,
  ) -> Result<(), SessionError> {
    self.update(move |transaction| {
      let active = transaction.load_generation(transaction.record.active)?;
      if active.id != expected_active {
        return Err(SessionError::StaleGeneration);
      }
      let original_count = transaction.store.list_len::<EntryId>(&active.entries)?;
      let list = transaction.allocate_list::<EntryId>()?;
      transaction.store.append_items(&list, &prefix)?;
      for message in body {
        let entry = transaction.store_entry(message, EntryOrigin::Summary)?;
        transaction.store.append_item(&list, &entry)?;
      }
      let compaction_cursor = transaction.store.list_len::<EntryId>(&list)?;
      if let Some(entry) = latest_user {
        transaction.store.append_item(&list, &entry)?;
      }
      let id =
        transaction.append_generation(GenerationStatus::Active, list, compaction_cursor, None)?;
      transaction.switch_active(id)?;
      // A session may previously have used local summaries. Keep its existing standby handle,
      // but discard those context references so a later local run starts from the new generation.
      let standby = transaction.load_generation(transaction.record.standby)?;
      if standby.source.is_some() || transaction.store.list_len::<EntryId>(&standby.entries)? != 0 {
        transaction.reset_standby()?;
      }
      transaction.record_event(SessionEvent::ContextCompacted {
        previous: active.id,
        active: id,
        reason,
        removed_entries: original_count - prefix.len() as u64 - u64::from(latest_user.is_some()),
        measurement,
      })?;
      Ok(())
    })
  }
}
