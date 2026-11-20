//! Complete message/event history, independent of the active context.
mod entry;
mod event;
mod index;
mod prune;
pub mod query;
mod reader;
pub use entry::{Entry, EntryId, EntryOrigin};
pub use event::SessionEvent;

use super::persistence::{SessionRecord, SessionTransaction};
use super::statistics::{ModelCallId, ModelCallRecord};
use super::{GenerationId, Session, SessionError};
use crate::protocol::Message;
use crate::utils::time::Timestamp;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum HistoryItem {
  Message(EntryId),
  Event(EventId),
}

/// Ordered facts, independent of generation positions. Sequence numbers start at zero.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct HistoryRecord {
  pub sequence: u64,
  pub generation: GenerationId,
  pub item: HistoryItem,
  /// When the fact was recorded; an event may carry its own earlier creation time.
  pub recorded_at: Timestamp,
  pub model_call_id: Option<ModelCallId>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EventId(pub u64);

/// A fact being recorded in history, with what it is about in hand for the index.
pub(in crate::session) enum Fact<'a> {
  Message(&'a Entry),
  Event(EventId, &'a SessionEvent),
}
impl Fact<'_> {
  fn item(&self) -> HistoryItem {
    match self {
      Self::Message(entry) => HistoryItem::Message(entry.id),
      Self::Event(id, _) => HistoryItem::Event(*id),
    }
  }
}

impl Session {
  /// Records events the executor observed at the times given, one history record each.
  pub(crate) fn record_events(
    &mut self,
    events: Vec<(Timestamp, SessionEvent)>,
  ) -> Result<(), SessionError> {
    if events.is_empty() {
      return Ok(());
    }
    self.update(move |transaction| {
      for (recorded_at, event) in events {
        transaction.recorded_at = recorded_at;
        transaction.record_event(event)?;
      }
      Ok(())
    })
  }
}
impl<R: std::ops::Deref<Target = SessionRecord>> SessionTransaction<'_, '_, R> {
  /// Records `event` and returns its id. It belongs to the running model call, unless it is one of
  /// the events no call causes.
  pub fn record_event(&mut self, event: SessionEvent) -> Result<EventId, SessionError> {
    let model_call_id = if event.belongs_to_call() { self.record.active_model_call } else { None };
    let id = EventId(self.store.append_item(&self.record.events, &event)?);
    self.record_history(Fact::Event(id, &event), model_call_id)?;
    Ok(id)
  }
  pub(in crate::session) fn record_history(
    &mut self,
    fact: Fact<'_>,
    model_call_id: Option<ModelCallId>,
  ) -> Result<(), SessionError> {
    let sequence = self.store.list_len::<HistoryRecord>(&self.record.history)?;
    let record = HistoryRecord {
      sequence,
      generation: self.record.active,
      item: fact.item(),
      recorded_at: self.recorded_at,
      model_call_id,
    };
    self.store.append_item(&self.record.history, &record)?;
    index::index_record(self.store, &self.record.history, &record, fact)
  }
  /// Stores `message` as a new entry, in no context and no history yet.
  pub fn store_entry(
    &mut self,
    message: Message,
    origin: EntryOrigin,
  ) -> Result<EntryId, SessionError> {
    Ok(self.create_entry(message, origin)?.id)
  }
  fn create_entry(
    &mut self,
    mut message: Message,
    origin: EntryOrigin,
  ) -> Result<Entry, SessionError> {
    let model_call_id = if origin.carries_call() { self.record.active_model_call } else { None };
    if let Message::Reasoning { source_model, .. } = &mut message
      && let Some(id) = model_call_id
      && let Some(call) = self.store.get_item::<ModelCallRecord>(&self.record.model_calls, id.0)?
    {
      *source_model = Some(call.model.clone());
    }
    let entry = Entry {
      id: EntryId(self.store.list_len::<Entry>(&self.record.entries)? as usize),
      origin,
      message,
      recorded_at: self.recorded_at,
      model_call_id,
    };
    self.store.append_item(&self.record.entries, &entry)?;
    Ok(entry)
  }
  /// Stores `message` as a new entry at the end of the active context, and records it in history.
  pub fn append_message(
    &mut self,
    message: Message,
    origin: EntryOrigin,
  ) -> Result<EntryId, SessionError> {
    let entry = self.create_entry(message, origin)?;
    let generation = self.load_generation(self.record.active)?;
    self.store.append_item(&generation.entries, &entry.id)?;
    self.record_history(Fact::Message(&entry), self.record.active_model_call)?;
    Ok(entry.id)
  }
}
