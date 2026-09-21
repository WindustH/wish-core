use crate::protocol::model_use::{
  request::{PromptCache, ReasoningConfig},
  tool::Tool,
};

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default)]
pub enum ToolMode {
  #[default]
  Serial,
  Parallel,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default)]
pub struct RunOptions {
  pub tools: ToolMode,
}
/// Session settings. Conversation is resolved from the active generation for each call.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
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
}

use super::{Session, SessionError, SessionEvent};
use serde_json::Value;
impl Session {
  pub fn get_metadata(&self) -> &Value {
    &self.record.metadata
  }
  pub fn set_metadata(&mut self, value: Value) -> Result<(), SessionError> {
    self.update(move |transaction| {
      transaction.record.metadata = value.clone();
      transaction.record_event(SessionEvent::MetadataUpdated(value))
    })
  }
  pub fn get_config(&self) -> &SessionConfig {
    &self.record.config
  }
  pub fn set_config(&mut self, config: SessionConfig) -> Result<(), SessionError> {
    self.require_stable()?;
    if let Some(compaction) = &config.compaction {
      compaction.validate()?;
    }
    self.update(move |transaction| {
      for id in [transaction.record.active, transaction.record.standby] {
        let mut generation = transaction.load_generation(id)?;
        generation.config = config.clone();
        transaction.save_generation(&generation)?;
      }
      transaction.record.config = config.clone();
      transaction.record_event(SessionEvent::ConfigUpdated(Box::new(config)))
    })
  }
}
