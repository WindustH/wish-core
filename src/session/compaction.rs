use super::*;
use crate::executor::model::tokens::{TokenEstimator, TokenMeasurement};

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
  pub(super) fn validate(&self) -> Result<(), SessionError> {
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
  pub(crate) fn start_compaction_call(&mut self) -> Result<(), SessionError> {
    self.update(|edit| edit.start_model_call(0, statistics::ModelCallPurpose::CompactionSummary))
  }
  pub(crate) fn complete_compaction_call(
    &mut self,
    observation: statistics::CallObservation,
    status: statistics::ModelCallStatus,
  ) -> Result<(), SessionError> {
    self.update(move |edit| edit.complete_model_call(observation, status))
  }
  pub(crate) fn begin_compaction(&mut self) -> Result<(), SessionError> {
    self.require_stable()?;
    self.update(|edit| {
      let resume = Box::new(edit.record.state.clone());
      edit.transition_to(SessionState::Compacting { resume })
    })
  }
  pub(crate) fn end_compaction(&mut self) -> Result<(), SessionError> {
    self.update(|edit| {
      let SessionState::Compacting { resume } = edit.record.state.clone() else {
        return Err(SessionError::Busy);
      };
      edit.record.active_model_call = None;
      edit.transition_to(*resume)
    })
  }
  pub(crate) fn save_compaction_summary(
    &mut self,
    generation: GenerationId,
    start: u64,
    end: u64,
    summary: Message,
    response: crate::protocol::Response,
  ) -> Result<(), SessionError> {
    self.update(move |edit| {
      let active = edit.load_generation(edit.record.active)?;
      if active.id != generation {
        return Err(SessionError::StaleGeneration);
      }
      let mut standby = edit.load_generation(edit.record.standby)?;
      let expected = standby
        .source
        .filter(|(id, _)| *id == active.id)
        .map(|(_, end)| end)
        .unwrap_or(active.compaction_cursor.max(start));
      if start != expected || end <= start || end > edit.tx.list_len::<EntryId>(&active.entries)? {
        return Err(SessionError::StaleGeneration);
      }
      if standby.source.is_none() {
        standby.entries = edit.create_list::<EntryId>()?;
        if start > 0 {
          let prefix = edit.tx.read_page::<EntryId>(&active.entries, 0, start as usize)?;
          let ids: Vec<_> = prefix.items.iter().map(|id| **id).collect();
          edit.tx.append_items(&standby.entries, &ids)?;
        }
      }
      let entry = edit.store_entry(summary, EntryOrigin::Summary)?;
      edit.tx.append_item(&standby.entries, &entry)?;
      standby.source = Some((active.id, end));
      standby.compaction_cursor = edit.tx.list_len::<EntryId>(&standby.entries)?;
      edit.save_generation(&standby)?;
      edit.record_event(SessionEvent::CompactionSummary {
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
    self.update(move |edit| {
      let mut active = edit.load_generation(edit.record.active)?;
      if active.id != generation {
        return Err(SessionError::StaleGeneration);
      }
      let mut standby = edit.load_generation(edit.record.standby)?;
      standby.entries = edit.create_list::<EntryId>()?;
      edit.tx.append_items(&standby.entries, &entries)?;
      standby.compaction_cursor = cursor;
      standby.source = None;
      standby.status = GenerationStatus::Active;
      active.status = GenerationStatus::Sealed;
      edit.save_generation(&active)?;
      edit.save_generation(&standby)?;
      edit.record.active = standby.id;
      let id = GenerationId(edit.tx.list_len::<Generation>(&edit.record.generations)? as usize);
      let seeded = edit.create_list::<EntryId>()?;
      edit.tx.append_items(&seeded, &entries[..cursor as usize])?;
      edit.tx.append_item(
        &edit.record.generations,
        &Generation {
          id,
          status: GenerationStatus::Standby,
          entries: seeded,
          config: edit.record.config.clone(),
          compaction_cursor: cursor,
          source: Some((standby.id, cursor)),
        },
      )?;
      edit.record.standby = id;
      edit.record_event(SessionEvent::GenerationActivated {
        previous: active.id,
        active: standby.id,
      })?;
      edit.record_event(SessionEvent::ContextCompacted {
        previous: active.id,
        active: standby.id,
        reason,
        removed_entries: removed,
        measurement,
      })
    })
  }
}
