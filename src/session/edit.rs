use super::*;
use crate::storage::{PAGE_SIZE, Transaction};

pub(super) struct SessionEdit<'a, 'db> {
  pub record: &'a mut SessionRecord,
  pub tx: &'a mut Transaction<'db>,
  pub key: &'a str,
  pub recorded_at: Timestamp,
}
impl SessionEdit<'_, '_> {
  pub fn create_list<T: crate::storage::StoredValue>(&mut self) -> Result<ListId, SessionError> {
    let id = self.record.next_list_id;
    self.record.next_list_id = id.checked_add(1).ok_or(StorageError::InvalidRange)?;
    let list = ListId(format!("{}/lists/{id}", self.key));
    self.tx.create_list::<T>(&list)?;
    Ok(list)
  }
  pub fn load_generation(&mut self, id: GenerationId) -> Result<Generation, SessionError> {
    self
      .tx
      .get_item::<Generation>(&self.record.generations, id.0 as u64)?
      .map(|item| (*item).clone())
      .ok_or_else(|| StorageError::Corrupt("missing generation".into()).into())
  }
  pub fn save_generation(&mut self, generation: &Generation) -> Result<(), SessionError> {
    Ok(self.tx.set_item(&self.record.generations, generation.id.0 as u64, generation)?)
  }
  pub fn record_event(&mut self, event: SessionEvent) -> Result<(), SessionError> {
    let model_call_id = match &event {
      SessionEvent::Created(_)
      | SessionEvent::ContextEntryCreated { .. }
      | SessionEvent::MetadataUpdated(_)
      | SessionEvent::ConfigUpdated(_)
      | SessionEvent::MessageQueued { .. }
      | SessionEvent::InputsConsumed { .. }
      | SessionEvent::GenerationPrepared { .. }
      | SessionEvent::GenerationActivated { .. } => None,
      _ => self.record.active_model_call,
    };
    let id = EventId(self.tx.append_item(&self.record.events, &event)?);
    self.record_history_with_call(HistoryItem::Event(id), model_call_id)
  }
  pub fn record_history(&mut self, item: HistoryItem) -> Result<(), SessionError> {
    self.record_history_with_call(item, self.record.active_model_call)
  }
  fn record_history_with_call(
    &mut self,
    item: HistoryItem,
    model_call_id: Option<ModelCallId>,
  ) -> Result<(), SessionError> {
    let sequence = self.tx.list_len::<HistoryRecord>(&self.record.history)?;
    self.tx.append_item(
      &self.record.history,
      &HistoryRecord {
        sequence,
        generation: self.record.active,
        item,
        recorded_at: Some(self.recorded_at),
        model_call_id,
      },
    )?;
    Ok(())
  }
  pub fn store_entry(
    &mut self,
    message: Message,
    origin: EntryOrigin,
  ) -> Result<EntryId, SessionError> {
    let id = EntryId(self.tx.list_len::<Entry>(&self.record.entries)? as usize);
    self.tx.append_item(
      &self.record.entries,
      &Entry {
        id,
        origin,
        message,
        recorded_at: Some(self.recorded_at),
        model_call_id: if matches!(
          origin,
          EntryOrigin::Model | EntryOrigin::Interrupted | EntryOrigin::Summary
        ) {
          self.record.active_model_call
        } else {
          None
        },
      },
    )?;
    Ok(id)
  }
  pub fn append_message(
    &mut self,
    message: Message,
    origin: EntryOrigin,
  ) -> Result<EntryId, SessionError> {
    let id = self.store_entry(message, origin)?;
    let generation = self.load_generation(self.record.active)?;
    self.tx.append_item(&generation.entries, &id)?;
    self.record_history(HistoryItem::Message(id))?;
    Ok(id)
  }
  pub fn transition_to(&mut self, next: SessionState) -> Result<(), SessionError> {
    let from = self.record.state.get_phase();
    let to = next.get_phase();
    self.record.state = next;
    self.record_event(SessionEvent::StateChanged { from, to })
  }
  pub fn append_input(&mut self, message: Message) -> Result<EntryId, SessionError> {
    let id = self.store_entry(message, EntryOrigin::Input)?;
    self.tx.append_item(&self.record.queue, &id)?;
    self.record_event(SessionEvent::MessageQueued { entry: id })?;
    Ok(id)
  }
  pub fn enqueue_message(&mut self, message: Message) -> Result<EntryId, SessionError> {
    let id = self.append_input(message)?;
    if matches!(self.record.state, SessionState::Idle) {
      self.transition_to(SessionState::Ready { completed_turns: 0, needs_model: true })?;
    }
    Ok(id)
  }
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

pub(super) fn validate_tool_pairs<'a>(
  messages: impl Iterator<Item = &'a Message>,
) -> Result<(), SessionError> {
  let mut validator = ToolPairValidator::default();
  for message in messages {
    validator.accept(message)?;
  }
  validator.finish()
}
#[derive(Default)]
pub(super) struct ToolPairValidator {
  pending: std::collections::HashMap<String, String>,
}
impl ToolPairValidator {
  pub fn accept(&mut self, message: &Message) -> Result<(), SessionError> {
    match message {
      Message::ToolUse { call_id, name, arguments, .. } => {
        if call_id.is_empty()
          || name.is_empty()
          || !arguments.is_object()
          || self.pending.insert(call_id.clone(), name.clone()).is_some()
        {
          return Err(SessionError::UnpairedTools);
        }
      }
      Message::ToolResult { call_id, name, .. } => {
        if self.pending.remove(call_id).as_ref() != Some(name) {
          return Err(SessionError::UnpairedTools);
        }
      }
      Message::User { .. } | Message::System { .. } | Message::Developer { .. }
        if !self.pending.is_empty() =>
      {
        return Err(SessionError::UnpairedTools);
      }
      _ => {}
    }
    Ok(())
  }
  pub fn finish(self) -> Result<(), SessionError> {
    if self.pending.is_empty() { Ok(()) } else { Err(SessionError::UnpairedTools) }
  }
}
