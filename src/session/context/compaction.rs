mod upstream;

use crate::executor::model::tokens::{TokenEstimator, TokenMeasurement};
use crate::protocol::Message;
use crate::session::{
  EntryId, EntryOrigin, Generation, GenerationId, GenerationStatus, Session, SessionError,
  SessionEvent,
};

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
  pub(crate) fn save_compaction_summary(
    &mut self,
    generation: GenerationId,
    start: u64,
    end: u64,
    summary: Message,
    response: crate::protocol::Response,
  ) -> Result<(), SessionError> {
    self.update(move |transaction| {
      let active = transaction.load_generation(transaction.record.active)?;
      if active.id != generation {
        return Err(SessionError::StaleGeneration);
      }
      let mut standby = transaction.load_generation(transaction.record.standby)?;
      let expected = standby
        .source
        .filter(|(id, _)| *id == active.id)
        .map(|(_, end)| end)
        .unwrap_or(active.compaction_cursor.max(start));
      if start != expected
        || end <= start
        || end > transaction.tx.list_len::<EntryId>(&active.entries)?
      {
        return Err(SessionError::StaleGeneration);
      }
      if standby.source.is_none() {
        standby.entries = transaction.create_list::<EntryId>()?;
        if start > 0 {
          let prefix = transaction.tx.read_page::<EntryId>(&active.entries, 0, start as usize)?;
          let ids: Vec<_> = prefix.items.iter().map(|id| **id).collect();
          transaction.tx.append_items(&standby.entries, &ids)?;
        }
      }
      let entry = transaction.store_entry(summary, EntryOrigin::Summary)?;
      transaction.tx.append_item(&standby.entries, &entry)?;
      standby.source = Some((active.id, end));
      standby.compaction_cursor = transaction.tx.list_len::<EntryId>(&standby.entries)?;
      transaction.save_generation(&standby)?;
      transaction.record_event(SessionEvent::CompactionSummary {
        generation,
        source_start: start,
        source_end: end,
        entry,
        response: Box::new(response),
      })
    })
  }
  /// Commits the already validated, measured context and its first unprocessed position together.
  pub(crate) fn commit_compaction(
    &mut self,
    generation: GenerationId,
    entries: Vec<EntryId>,
    cursor: u64,
    removed: u64,
    measurement: TokenMeasurement,
    reason: CompactionReason,
  ) -> Result<(), SessionError> {
    self.update(move |transaction| {
      let mut active = transaction.load_generation(transaction.record.active)?;
      if active.id != generation {
        return Err(SessionError::StaleGeneration);
      }
      let mut standby = transaction.load_generation(transaction.record.standby)?;
      standby.entries = transaction.create_list::<EntryId>()?;
      transaction.tx.append_items(&standby.entries, &entries)?;
      standby.compaction_cursor = cursor;
      standby.source = None;
      standby.status = GenerationStatus::Active;
      active.status = GenerationStatus::Sealed;
      transaction.save_generation(&active)?;
      transaction.save_generation(&standby)?;
      transaction.record.active = standby.id;
      let id = GenerationId(
        transaction.tx.list_len::<Generation>(&transaction.record.generations)? as usize,
      );
      let seeded = transaction.create_list::<EntryId>()?;
      transaction.tx.append_items(&seeded, &entries[..cursor as usize])?;
      transaction.tx.append_item(
        &transaction.record.generations,
        &Generation {
          id,
          status: GenerationStatus::Standby,
          entries: seeded,
          config: transaction.record.config.clone(),
          compaction_cursor: cursor,
          source: Some((standby.id, cursor)),
        },
      )?;
      transaction.record.standby = id;
      transaction.record_event(SessionEvent::GenerationActivated {
        previous: active.id,
        active: standby.id,
      })?;
      transaction.record_event(SessionEvent::ContextCompacted {
        previous: active.id,
        active: standby.id,
        reason,
        removed_entries: removed,
        measurement,
      })
    })
  }
}
