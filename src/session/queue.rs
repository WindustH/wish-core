//! Durable input: the queue senders append to, reordering and cancelling what waits in it, and its
//! consumption into the active context at a stable boundary.
use super::context::is_input;
use super::history::Fact;
use super::persistence::SessionTransaction;
use super::{EntryId, EntryOrigin, EventId, Session, SessionError, SessionEvent, SessionState};
use crate::{protocol::Message, storage::Storage};
use serde_json::Value;
use tokio::sync::watch;

/// A durable input handle usable while the runner owns the session, and after it is gone. Sending
/// commits the entry, queue reference and history event together; it never interrupts or changes a
/// running phase. A sender never changes the session record (see [`SessionSender::transact`]).
#[derive(Clone)]
pub struct SessionSender {
  pub(super) storage: Storage,
  pub(super) key: String,
  arrivals: InputArrivals,
}
impl SessionSender {
  /// Appends `message` to the queue, where the next run collects it.
  pub fn enqueue_message(&self, message: Message) -> Result<EntryId, SessionError> {
    if !is_input(&message) {
      return Err(SessionError::InvalidInput);
    }
    let entry = self.transact(move |transaction| transaction.append_input(message))?;
    self.arrivals.announce();
    Ok(entry)
  }
  /// Records a note of the application's in the session's history, beside whatever runs.
  pub fn record_application_event(&self, value: Value) -> Result<EventId, SessionError> {
    self.transact(move |transaction| transaction.record_event(SessionEvent::Application(value)))
  }
  /// Move a pending input before another pending input, or to the end.
  /// Validation and writes share one transaction with enqueue/consume/cancel.
  pub fn move_queued_input(
    &self,
    entry: EntryId,
    before: Option<EntryId>,
  ) -> Result<(), SessionError> {
    self.transact(move |transaction| {
      let queue = transaction.record.queue.clone();
      let head = transaction.record.queue_head;
      let original: Vec<EntryId> =
        transaction.store.read_from::<EntryId>(&queue, head)?.iter().map(|id| **id).collect();
      let source =
        original.iter().position(|id| *id == entry).ok_or(SessionError::InvalidEntry(entry))?;
      if let Some(target) = before
        && !original.contains(&target)
      {
        return Err(SessionError::InvalidEntry(target));
      }
      if before == Some(entry) {
        return Ok(());
      }
      let mut pending = original.clone();
      pending.remove(source);
      let destination = before
        .and_then(|target| pending.iter().position(|id| *id == target))
        .unwrap_or(pending.len());
      pending.insert(destination, entry);
      if pending == original {
        return Ok(());
      }
      for (offset, id) in pending.iter().enumerate() {
        if *id != original[offset] {
          transaction.store.set_item(&queue, head + offset as u64, id)?;
        }
      }
      transaction.record_event(SessionEvent::InputMoved { entry, before })?;
      Ok(())
    })
  }
  /// Atomically remove a pending input, including while a model or tool is running.
  /// A consumed entry cannot be removed; its original content always remains stored.
  pub fn cancel_queued_input(&self, entry: EntryId) -> Result<(), SessionError> {
    self.transact(move |transaction| {
      let queue = transaction.record.queue.clone();
      let head = transaction.record.queue_head;
      let pending = transaction.store.read_from::<EntryId>(&queue, head)?;
      let offset =
        pending.iter().position(|id| **id == entry).ok_or(SessionError::InvalidEntry(entry))?;
      transaction.store.remove_item::<EntryId>(&queue, head + offset as u64)?;
      transaction.record_event(SessionEvent::InputCancelled { entry })?;
      Ok(())
    })
  }
}

/// Tells the owner's executor that input was queued. The queue stays the durable record; this only
/// wakes an executor that waits on background work, so it can collect the input at once.
#[derive(Clone)]
pub(super) struct InputArrivals(watch::Sender<()>);
impl Default for InputArrivals {
  fn default() -> Self {
    Self(watch::channel(()).0)
  }
}
impl InputArrivals {
  fn announce(&self) {
    self.0.send_replace(());
  }
}

impl Session {
  /// Positions below this have been consumed; the queue log stays addressable.
  pub fn get_queue_head(&self) -> u64 {
    self.record.queue_head
  }
  pub fn create_sender(&self) -> SessionSender {
    SessionSender {
      storage: self.storage.clone(),
      key: self.key.clone(),
      arrivals: self.arrivals.clone(),
    }
  }
  /// Watches for input queued through this owner's senders from now on.
  pub(crate) fn watch_input_arrivals(&self) -> watch::Receiver<()> {
    self.arrivals.0.subscribe()
  }
  /// Whether queued input is waiting to be collected.
  pub(crate) fn has_queued_input(&self) -> Result<bool, SessionError> {
    Ok(self.reader.get_message_queue().len()? > self.record.queue_head)
  }
  /// Notice durable queued input at a stable boundary. Already-running phases are unchanged.
  pub fn collect_inputs(&mut self) -> Result<(), SessionError> {
    if !matches!(self.record.state, SessionState::Idle) || !self.has_queued_input()? {
      return Ok(());
    }
    self.update(move |transaction| {
      if transaction.record.queue_head
        < transaction.store.list_len::<EntryId>(&transaction.record.queue)?
      {
        transaction.transition_to(SessionState::Ready { completed_turns: 0, needs_model: true })?;
      }
      Ok(())
    })
  }
}
impl SessionTransaction<'_, '_> {
  fn append_input(&mut self, message: Message) -> Result<EntryId, SessionError> {
    let entry = self.store_entry(message, EntryOrigin::Input)?;
    self.store.append_item(&self.record.queue, &entry)?;
    self.record_event(SessionEvent::MessageQueued { entry })?;
    Ok(entry)
  }
  /// Moves the queued input, up to `queue_end`, the end of the queue, into the active context and
  /// its history.
  pub(super) fn consume_inputs(&mut self, queue_end: u64) -> Result<(), SessionError> {
    let queue_start = self.record.queue_head;
    let generation = self.load_generation(self.record.active)?;
    for id in self.store.read_from::<EntryId>(&self.record.queue, queue_start)? {
      let entry = self.load_entry(*id)?;
      self.store.append_item(&generation.entries, &entry.id)?;
      self.record_history(Fact::Message(&entry), self.record.active_model_call)?;
    }
    self.record.queue_head = queue_end;
    if queue_start != queue_end {
      self.record_event(SessionEvent::InputsConsumed { queue_start, queue_end })?;
    }
    Ok(())
  }
}
