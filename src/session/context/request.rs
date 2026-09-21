use crate::protocol::Request;
use crate::session::persistence::SessionTransaction;
use crate::session::statistics::Timestamp;
use crate::session::{Entry, EntryId, Session, SessionError};
use crate::storage::PAGE_SIZE;

impl Session {
  pub fn build_request(&self) -> Result<Request, SessionError> {
    let recorded_at = Timestamp::now();
    let mut record = self.record.clone();
    let key = self.key.clone();
    self.storage.transaction(move |tx| {
      SessionTransaction { record: &mut record, tx, key: &key, recorded_at }.build_request()
    })
  }
}
impl SessionTransaction<'_, '_> {
  pub fn build_request(&mut self) -> Result<Request, SessionError> {
    let generation = self.load_generation(self.record.active)?;
    let length = self.tx.list_len::<EntryId>(&generation.entries)?;
    let mut messages = Vec::new();
    let mut start = 0;
    while start < length {
      let page = self.tx.read_page::<EntryId>(&generation.entries, start, PAGE_SIZE as usize)?;
      for id in &page.items {
        let entry = self
          .tx
          .get_item::<Entry>(&self.record.entries, id.0 as u64)?
          .ok_or(SessionError::InvalidEntry(**id))?;
        messages.push(entry.message.clone());
      }
      start += page.items.len() as u64;
    }
    Ok(self.record.config.build_request(messages))
  }
}

use crate::protocol::{Message, model_use::request::ToolChoice};
use crate::session::SessionConfig;
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
