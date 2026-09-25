//! Complete message/event history, independent of the active context.
mod entry;
mod index;
pub mod query;
mod reader;
pub use reader::HistoryReader;
mod event;
pub use entry::{Entry, EntryId, EntryOrigin};
pub use event::SessionEvent;

use super::persistence::SessionTransaction;
use super::statistics::{ModelCallId, Timestamp};
use super::{GenerationId, Session, SessionError};
use crate::protocol::Message;
use crate::storage::ReadList;
use std::sync::Arc;

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
impl Session {
  pub fn get_history(&self) -> ReadList<HistoryRecord> {
    self.storage.open_list(&self.record.history).read_only()
  }
  pub fn get_entries(&self) -> ReadList<Entry> {
    self.storage.open_list(&self.record.entries).read_only()
  }
  pub fn get_events(&self) -> ReadList<SessionEvent> {
    self.storage.open_list(&self.record.events).read_only()
  }
  pub fn get_entry(&self, id: EntryId) -> Result<Option<Arc<Entry>>, SessionError> {
    Ok(self.get_entries().get(id.0 as u64)?)
  }
  pub fn get_event(&self, id: EventId) -> Result<Option<Arc<SessionEvent>>, SessionError> {
    Ok(self.get_events().get(id.0)?)
  }
  pub(crate) fn record_events(
    &mut self,
    events: Vec<(Timestamp, SessionEvent)>,
  ) -> Result<(), SessionError> {
    if events.is_empty() {
      return Ok(());
    }
    let record = self.record.clone();
    self.storage.transaction(move |tx| -> Result<(), SessionError> {
      let (timestamps, events): (Vec<_>, Vec<_>) = events.into_iter().unzip();
      super::statistics::record_stream_usage(tx, &record, &timestamps, &events)?;
      let first_event = tx.append_items(&record.events, &events)?;
      let first_sequence = tx.list_len::<HistoryRecord>(&record.history)?;
      let history: Vec<_> = events
        .iter()
        .enumerate()
        .map(|(offset, _)| HistoryRecord {
          recorded_at: timestamps[offset],
          model_call_id: record.active_model_call,
          sequence: first_sequence + offset as u64,
          generation: record.active,
          item: HistoryItem::Event(EventId(first_event + offset as u64)),
        })
        .collect();
      tx.append_items(&record.history, &history)?;
      for (item, event) in history.iter().zip(&events) {
        index::index_event_record(tx, &record.history, item, event)?;
      }
      Ok(())
    })
  }
}
impl SessionTransaction<'_, '_> {
  pub fn record_event(&mut self, event: SessionEvent) -> Result<(), SessionError> {
    let model_call_id = match &event {
      SessionEvent::Created(_)
      | SessionEvent::ContextEntryCreated { .. }
      | SessionEvent::MetadataUpdated(_)
      | SessionEvent::ConfigUpdated(_)
      | SessionEvent::InputMoved { .. }
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
  pub(in crate::session) fn record_history_with_call(
    &mut self,
    item: HistoryItem,
    model_call_id: Option<ModelCallId>,
  ) -> Result<(), SessionError> {
    let sequence = self.tx.list_len::<HistoryRecord>(&self.record.history)?;
    let record = HistoryRecord {
      sequence,
      generation: self.record.active,
      item,
      recorded_at: self.recorded_at,
      model_call_id,
    };
    self.tx.append_item(&self.record.history, &record)?;
    index::index_record(
      self.tx,
      &self.record.history,
      &self.record.entries,
      &self.record.events,
      &record,
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
        recorded_at: self.recorded_at,
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
}
