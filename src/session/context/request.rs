use super::validate_tool_pairs;
use crate::protocol::{Message, Request, model_use::request::ToolChoice};
use crate::session::persistence::{SessionRecord, SessionTransaction};
use crate::session::statistics::ModelCallRecord;
use crate::session::{EntryId, Session, SessionConfig, SessionError, SessionSender, SessionState};
use crate::utils::time::Timestamp;

impl Session {
  pub fn build_request(&self) -> Result<Request, SessionError> {
    self.read(|transaction| transaction.build_request())
  }
  /// The same request projection for a compaction candidate, including legacy reasoning provenance.
  pub fn build_request_from_entries(&self, entries: Vec<EntryId>) -> Result<Request, SessionError> {
    self.read(move |transaction| transaction.build_request_from_entries(entries))
  }
}
impl SessionTransaction<'_, '_> {
  /// The request the active context makes, with the session's settings.
  pub fn build_request(&mut self) -> Result<Request, SessionError> {
    let generation = self.load_generation(self.record.active)?;
    let entries = self.store.read_from::<EntryId>(&generation.entries, 0)?;
    self.build_request_from_entries(entries.iter().map(|id| **id).collect())
  }
  fn build_request_from_entries(&mut self, entries: Vec<EntryId>) -> Result<Request, SessionError> {
    let mut messages = Vec::new();
    for id in entries {
      let entry = self.load_entry(id)?;
      let mut message = entry.message.clone();
      // Old entries already retain their call id. Recover the model in this read-only
      // projection so retries, context inspection and forks need no history migration.
      if let Message::Reasoning { source_model, .. } = &mut message
        && source_model.is_none()
        && let Some(id) = entry.model_call_id
        && let Some(call) =
          self.store.get_item::<ModelCallRecord>(&self.record.model_calls, id.0)?
      {
        *source_model = Some(call.model.clone());
      }
      messages.push(message);
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
