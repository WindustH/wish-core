use crate::protocol::model_use::{
  request::{PromptCache, ReasoningConfig, ToolChoice},
  tool::Tool,
};
use crate::protocol::{Message, Request};

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default)]
pub enum ToolMode {
  #[default]
  Serial,
  Parallel,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
pub struct RunOptions {
  pub max_turns: usize,
  pub tools: ToolMode,
}
impl Default for RunOptions {
  fn default() -> Self {
    Self { max_turns: 32, tools: ToolMode::Serial }
  }
}

/// Session settings. Conversation is resolved from the active generation for each call.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct SessionConfig {
  pub model: String,
  pub stream: bool,
  pub tools: Vec<Tool>,
  pub tool_choice: Option<ToolChoice>,
  pub max_output_tokens: Option<u64>,
  pub reasoning: Option<ReasoningConfig>,
  pub cache: Option<PromptCache>,
  pub run: RunOptions,
}

impl SessionConfig {
  pub fn new(model: impl Into<String>) -> Self {
    Self {
      model: model.into(),
      stream: true,
      tools: Vec::new(),
      tool_choice: None,
      max_output_tokens: None,
      reasoning: None,
      cache: None,
      run: RunOptions::default(),
    }
  }

  pub(crate) fn build_request(&self, conversation: Vec<Message>) -> Request {
    Request {
      model: self.model.clone(),
      stream: self.stream,
      tools: self.tools.clone(),
      tool_choice: self.tool_choice,
      max_output_tokens: self.max_output_tokens,
      reasoning: self.reasoning.clone(),
      cache: self.cache.clone(),
      conversation,
    }
  }
}
