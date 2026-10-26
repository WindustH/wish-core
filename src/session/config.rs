use super::{Session, SessionError, SessionEvent};
use crate::protocol::model_use::{
  request::{PromptCache, ReasoningConfig},
  tool::Tool,
};
use serde_json::Value;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default)]
pub enum ToolMode {
  Serial,
  #[default]
  Parallel,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct RunOptions {
  pub tools: ToolMode,
  /// Yield running tools to background owners when actionable input is queued.
  #[serde(default)]
  pub background_tools_on_input: bool,
}
/// Session settings. Conversation is resolved from the active generation for each call.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
  pub model: String,
  pub stream: bool,
  pub tools: Vec<Tool>,
  pub max_output_tokens: Option<u64>,
  pub reasoning: Option<ReasoningConfig>,
  pub cache: Option<PromptCache>,
  pub run: RunOptions,
  #[serde(default)]
  pub compaction: Option<super::CompactionConfig>,
}

impl SessionConfig {
  pub fn new(model: impl Into<String>) -> Self {
    Self {
      model: model.into(),
      stream: true,
      tools: Vec::new(),
      max_output_tokens: None,
      reasoning: None,
      cache: None,
      run: RunOptions::default(),
      compaction: None,
    }
  }
  /// Refuses settings no session can run with.
  pub(in crate::session) fn validate(&self) -> Result<(), SessionError> {
    match &self.compaction {
      Some(compaction) => compaction.validate(),
      None => Ok(()),
    }
  }
}

impl Session {
  pub fn get_metadata(&self) -> &Value {
    &self.record.metadata
  }
  pub fn set_metadata(&mut self, value: Value) -> Result<(), SessionError> {
    self.update(move |transaction| {
      transaction.record.metadata = value.clone();
      transaction.record_event(SessionEvent::MetadataUpdated(value))?;
      Ok(())
    })
  }
  pub fn get_config(&self) -> &SessionConfig {
    &self.record.config
  }
  pub fn set_config(&mut self, config: SessionConfig) -> Result<(), SessionError> {
    self.require_stable()?;
    config.validate()?;
    self.update(move |transaction| {
      for id in [transaction.record.active, transaction.record.standby] {
        let mut generation = transaction.load_generation(id)?;
        generation.config = config.clone();
        transaction.save_generation(&generation)?;
      }
      transaction.record.config = config.clone();
      transaction.record_event(SessionEvent::ConfigUpdated(Box::new(config)))?;
      Ok(())
    })
  }
}

impl Session {
  /// Keep the active prefix while making a tool change explicit to the model. The exact
  /// target tool set and instruction are installed at the next generation boundary.
  pub fn defer_instruction_config(
    &mut self,
    source: String,
    text: String,
    notice: String,
    immediate: SessionConfig,
    next_tools: Vec<Tool>,
  ) -> Result<(), SessionError> {
    self.require_stable()?;
    immediate.validate()?;
    self.update(move |tx| {
      tx.record.next_generation_instructions.insert(source.clone(), text.clone());
      tx.record.next_generation_tools = Some(next_tools);
      tx.record.config = immediate.clone();
      for id in [tx.record.active, tx.record.standby] {
        let mut generation = tx.load_generation(id)?;
        generation.config = immediate.clone();
        tx.save_generation(&generation)?;
      }
      tx.append_input(crate::protocol::Message::System {
        metadata: serde_json::json!({"next_generation_instruction":{"source":source,"text":text}}),
        content: vec![crate::protocol::ContentBlock::Text { text: notice }],
      })?;
      tx.record_event(SessionEvent::ConfigUpdated(Box::new(immediate)))?;
      Ok(())
    })
  }
  pub fn has_deferred_instruction(&self, source: &str) -> bool {
    self.record.next_generation_instructions.contains_key(source)
  }
  /// Persist a managed system instruction before use. Changed instructions start a new
  /// generation, preserving the previous generation's message references and configuration.
  pub fn set_system_instruction_config(
    &mut self,
    source: String,
    text: String,
    config: SessionConfig,
  ) -> Result<(), SessionError> {
    use super::{EntryId, EntryOrigin, GenerationStatus};
    use crate::protocol::{ContentBlock, Message};
    self.require_stable()?;
    config.validate()?;
    self.update(move |tx| {
      let active = tx.load_generation(tx.record.active)?;
      let ids = tx.store.read_from::<EntryId>(&active.entries, 0)?;
      let desired = Message::System {
        metadata: serde_json::json!({"source":source}),
        content: vec![ContentBlock::Text { text }],
      };
      let mut positions = Vec::new();
      let mut changed = false;
      for (position, id) in ids.iter().enumerate() {
        let entry = tx.load_entry(**id)?;
        if let Message::System { metadata, .. } = &entry.message
          && metadata["source"] == source
        {
          positions.push(position);
          changed |=
            serde_json::to_value(&entry.message).ok() != serde_json::to_value(&desired).ok();
        }
      }
      let config_changed =
        serde_json::to_value(&tx.record.config).ok() != serde_json::to_value(&config).ok();
      if changed || positions.is_empty() {
        let entry = tx.store_entry(desired, EntryOrigin::Context)?;
        let list = tx.allocate_list::<EntryId>()?;
        if positions.is_empty() {
          tx.store.append_item(&list, &entry)?;
        }
        for (position, id) in ids.iter().enumerate() {
          tx.store.append_item(&list, &if positions.contains(&position) { entry } else { **id })?;
        }
        tx.record.config = config.clone();
        let cursor = active.compaction_cursor
          + u64::from(positions.is_empty() && active.compaction_cursor > 0);
        let next = tx.append_generation(GenerationStatus::Active, list, cursor, None)?;
        tx.switch_active(next)?;
        let stored = tx.load_entry(entry)?;
        tx.record_history(super::history::Fact::Message(&stored), None)?;
        tx.reset_standby()?;
      } else if config_changed {
        tx.record.config = config.clone();
        for id in [tx.record.active, tx.record.standby] {
          let mut generation = tx.load_generation(id)?;
          generation.config = config.clone();
          tx.save_generation(&generation)?;
        }
      }
      if config_changed {
        tx.record_event(SessionEvent::ConfigUpdated(Box::new(config)))?;
      }
      Ok(())
    })
  }
}

#[cfg(test)]
mod instruction_tests {
  use super::*;
  use crate::{
    protocol::{ContentBlock, Message},
    session::{EntryOrigin, GenerationStatus},
  };
  #[test]
  fn persisted_instruction_update_preserves_previous_generation() {
    let config = SessionConfig::new("test");
    let mut session = Session::new(config.clone()).unwrap();
    session
      .import_history(vec![(
        Message::System {
          metadata: serde_json::json!({"source":"agent"}),
          content: vec![ContentBlock::Text { text: "old".into() }],
        },
        EntryOrigin::Imported,
      )])
      .unwrap();
    let old = session.get_active_generation().unwrap();
    session
      .set_system_instruction_config("agent".into(), "complete instructions".into(), config.clone())
      .unwrap();
    let current = session.get_active_generation().unwrap();
    assert_ne!(old.id, current.id);
    assert_eq!(session.reader().get_generation(old.id).unwrap().status, GenerationStatus::Sealed);
    let ids = session.reader().read_generation_entry_ids(old.id).unwrap();
    let entry = session.reader().get_entries().get(ids[0].0 as u64).unwrap().unwrap();
    assert!(serde_json::to_string(&entry.message).unwrap().contains("old"));
    assert!(
      serde_json::to_string(&session.build_request().unwrap().conversation)
        .unwrap()
        .contains("complete instructions")
    );
    session
      .set_system_instruction_config("agent".into(), "complete instructions".into(), config)
      .unwrap();
    assert_eq!(session.get_active_generation().unwrap().id, current.id);
  }
}
