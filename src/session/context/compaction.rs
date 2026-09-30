mod tokens;
mod translation;
mod upstream;

use crate::protocol::Message;
use crate::session::statistics::ModelCallId;
use crate::session::{EntryId, EntryOrigin, GenerationId, Session, SessionError, SessionEvent};
pub use tokens::{TokenEstimator, TokenMeasurement, TokenMeasurementSource};

/// Explicit token budgets; absence from SessionConfig disables automatic compaction.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CompactionConfig {
  pub trigger_tokens: u64,
  pub target_tokens: u64,
  /// Prepare another standby summary once this much unprocessed context has accumulated.
  pub segment_tokens: u64,
  #[serde(default)]
  pub estimator: TokenEstimator,
}
impl CompactionConfig {
  pub(in crate::session) fn validate(&self) -> Result<(), SessionError> {
    if self.target_tokens == 0
      || self.target_tokens >= self.trigger_tokens
      || self.segment_tokens == 0
    {
      return Err(SessionError::InvalidCompaction(
        "require 0 < target_tokens < trigger_tokens and segment_tokens > 0".into(),
      ));
    }
    if !self.estimator.bytes_per_token.is_finite() || self.estimator.bytes_per_token <= 0.0 {
      return Err(SessionError::InvalidCompaction(
        "bytes_per_token must be finite and positive".into(),
      ));
    }
    Ok(())
  }
}
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum CompactionReason {
  Usage,
  ContextRejected,
  Manual,
}

impl Session {
  /// Appends a standby summary of the span `start..end` of the active generation, which must still
  /// be `expected_active`. The first summary of a standby starts it anew, over the active prefix
  /// before the span; each later one continues where the one before it ended.
  pub(crate) fn append_standby_summary(
    &mut self,
    expected_active: GenerationId,
    start: u64,
    end: u64,
    summary: Message,
    response: crate::protocol::Response,
    call: ModelCallId,
  ) -> Result<(), SessionError> {
    self.update(move |transaction| {
      let active = transaction.load_generation(transaction.record.active)?;
      if active.id != expected_active {
        return Err(SessionError::StaleGeneration);
      }
      let mut standby = transaction.load_generation(transaction.record.standby)?;
      let expected = standby.summarized_end(&active).unwrap_or(active.compaction_cursor.max(start));
      if start != expected
        || end <= start
        || end > transaction.store.list_len::<EntryId>(&active.entries)?
      {
        return Err(SessionError::StaleGeneration);
      }
      if standby.source.is_none() {
        transaction.replace_entries(&mut standby)?;
        if start > 0 {
          let prefix =
            transaction.store.read_page::<EntryId>(&active.entries, 0, start as usize)?;
          let ids: Vec<_> = prefix.items.iter().map(|id| **id).collect();
          transaction.store.append_items(&standby.entries, &ids)?;
        }
      }
      // The summary and its event belong to the summary's own call, not to the conversation
      // call that may be running beside it.
      transaction.with_model_call(call, |transaction| {
        let entry = transaction.store_entry(summary, EntryOrigin::Summary)?;
        transaction.store.append_item(&standby.entries, &entry)?;
        standby.source = Some((active.id, end));
        standby.compaction_cursor = transaction.store.list_len::<EntryId>(&standby.entries)?;
        transaction.save_generation(&standby)?;
        transaction.record_event(SessionEvent::CompactionSummary {
          generation: expected_active,
          source_start: start,
          source_end: end,
          entry,
          response: Box::new(response),
        })?;
        Ok(())
      })
    })
  }
  /// Commits a local cutover: the standby becomes the active generation holding `entries`, which
  /// are validated and measured already, with its first unsummarized position; a new standby
  /// starts from the entries before it.
  pub(crate) fn commit_local_compaction(
    &mut self,
    expected_active: GenerationId,
    entries: Vec<EntryId>,
    compaction_cursor: u64,
    removed: u64,
    measurement: TokenMeasurement,
    reason: CompactionReason,
  ) -> Result<(), SessionError> {
    self.update(move |transaction| {
      if transaction.record.active != expected_active {
        return Err(SessionError::StaleGeneration);
      }
      let mut standby = transaction.load_generation(transaction.record.standby)?;
      transaction.replace_entries(&mut standby)?;
      transaction.store.append_items(&standby.entries, &entries)?;
      standby.compaction_cursor = compaction_cursor;
      transaction.save_generation(&standby)?;
      let previous = transaction.promote_standby()?;
      let seed = &entries[..compaction_cursor as usize];
      transaction.append_standby(seed, compaction_cursor, Some((standby.id, compaction_cursor)))?;
      transaction.record_event(SessionEvent::ContextCompacted {
        previous,
        active: standby.id,
        reason,
        removed_entries: removed,
        measurement,
      })?;
      Ok(())
    })
  }
}
