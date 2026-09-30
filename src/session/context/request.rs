use super::validate_tool_pairs;
use crate::protocol::{Message, Request, model_use::request::ToolChoice};
use crate::session::persistence::{SessionRecord, SessionTransaction};
use crate::session::{EntryId, Session, SessionConfig, SessionError, SessionSender, SessionState};
use crate::utils::time::Timestamp;

impl Session {
  pub fn build_request(&self) -> Result<Request, SessionError> {
    self.read(|transaction| transaction.build_request())
  }
}
impl SessionTransaction<'_, '_> {
  /// The request the active context makes, with the session's settings.
  pub fn build_request(&mut self) -> Result<Request, SessionError> {
    let generation = self.load_generation(self.record.active)?;
    let mut messages = Vec::new();
    for id in self.store.read_from::<EntryId>(&generation.entries, 0)? {
      messages.push(self.load_entry(*id)?.message.clone());
    }
    Ok(self.record.config.build_request(messages))
  }
}

impl SessionConfig {
  pub(crate) fn build_request(&self, conversation: Vec<Message>) -> Request {
    Request {
      model: self.model.clone(),
      stream: self.stream,
      tools: self.tools.clone(),
      tool_choice: Some(ToolChoice::Auto),
      max_output_tokens: self.max_output_tokens,
      reasoning: self.reasoning.clone(),
      cache: self.cache.clone(),
      conversation,
    }
  }
}

impl SessionSender {
  /// Atomically snapshot committed context without borrowing the running session.
  /// An in-flight tool batch is excluded as a whole, including its assistant turn.
  pub fn build_context_snapshot(&self) -> Result<Request, SessionError> {
    let key = self.key.clone();
    self.storage.rehearse(move |store| {
      let mut record = (*store.load_object::<SessionRecord>(&key)?).clone();
      let pending_tools = matches!(record.state, SessionState::ExecutingTools { .. });
      let mut request =
        SessionTransaction { record: &mut record, store, key: &key, recorded_at: Timestamp::now() }
          .build_request()?;
      if pending_tools {
        while matches!(
          request.conversation.last(),
          Some(Message::Assistant { .. } | Message::Reasoning { .. } | Message::ToolUse { .. })
        ) {
          request.conversation.pop();
        }
      }
      validate_tool_pairs(request.conversation.iter())?;
      Ok(request)
    })
  }
}
