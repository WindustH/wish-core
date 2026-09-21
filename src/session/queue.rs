use super::{
  EntryId, EntryOrigin, HistoryItem, Session, SessionError, SessionEvent, SessionState,
  persistence::{SessionRecord, SessionTransaction},
};
use crate::{
  protocol::Message,
  storage::{PAGE_SIZE, ReadList, Storage},
};

/// A durable input handle usable while the runner owns the session. Sending commits the entry,
/// queue reference and history event together; it never interrupts or changes a running phase.
#[derive(Clone)]
pub struct SessionSender {
  pub(crate) storage: Storage,
  pub(crate) key: String,
}
impl SessionSender {
  pub fn enqueue_message(&self, message: Message) -> Result<EntryId, SessionError> {
    validate_input(&message)?;
    let key = self.key.clone();
    let recorded_at = crate::session::statistics::Timestamp::now();
    self.storage.transaction(move |tx| {
      let stored = tx.load_object::<SessionRecord>(&key)?;
      let mut record = (*stored).clone();
      SessionTransaction { record: &mut record, tx, key: &key, recorded_at }.append_input(message)
    })
  }
}

pub(crate) fn validate_input(message: &Message) -> Result<(), SessionError> {
  if matches!(message, Message::User { .. } | Message::System { .. } | Message::Developer { .. }) {
    Ok(())
  } else {
    Err(SessionError::InvalidInput)
  }
}

impl Session {
  /// The queue log is paged too. Positions below get_queue_head() have already been consumed.
  pub fn get_message_queue(&self) -> ReadList<EntryId> {
    self.storage.open_list(&self.record.queue).read_only()
  }
  pub fn get_queue_head(&self) -> u64 {
    self.record.queue_head
  }
  pub fn create_sender(&self) -> SessionSender {
    SessionSender { storage: self.storage.clone(), key: self.key.clone() }
  }
  pub fn enqueue_message(&mut self, message: Message) -> Result<EntryId, SessionError> {
    validate_input(&message)?;
    self.update(move |transaction| transaction.enqueue_message(message))
  }
  /// Notice durable queued input at a stable boundary. Already-running phases are unchanged.
  pub fn collect_inputs(&mut self) -> Result<(), SessionError> {
    if !matches!(self.record.state, SessionState::Idle) {
      return Ok(());
    }
    if self.get_message_queue().len()? == self.record.queue_head {
      return Ok(());
    }
    self.update(move |transaction| {
      if transaction.record.queue_head
        < transaction.tx.list_len::<EntryId>(&transaction.record.queue)?
      {
        transaction.transition_to(SessionState::Ready { completed_turns: 0, needs_model: true })?;
      }
      Ok(())
    })
  }
}
impl SessionTransaction<'_, '_> {
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
  pub(super) fn consume_inputs(&mut self, queue_end: u64) -> Result<(), SessionError> {
    let queue_start = self.record.queue_head;
    let generation = self.load_generation(self.record.active)?;
    while self.record.queue_head < queue_end {
      let page = self.tx.read_page::<EntryId>(
        &self.record.queue,
        self.record.queue_head,
        PAGE_SIZE as usize,
      )?;
      for id in &page.items {
        self.tx.append_item(&generation.entries, id.as_ref())?;
        self.record_history(HistoryItem::Message(**id))?;
      }
      self.record.queue_head += page.items.len() as u64;
    }
    if queue_start != queue_end {
      self.record_event(SessionEvent::InputsConsumed { queue_start, queue_end })?;
    }
    Ok(())
  }
}

impl SessionSender {
  /// Move a pending input before another pending input, or to the end.
  /// Validation and writes share one transaction with enqueue/consume/cancel.
  pub fn move_queued_input(
    &self,
    entry: EntryId,
    before: Option<EntryId>,
  ) -> Result<(), SessionError> {
    let key = self.key.clone();
    let recorded_at = crate::session::statistics::Timestamp::now();
    self.storage.transaction(move |tx| {
      let stored = tx.load_object::<SessionRecord>(&key)?;
      let mut record = (*stored).clone();
      let mut position = record.queue_head;
      let end = tx.list_len::<EntryId>(&record.queue)?;
      let mut pending = Vec::new();
      while position < end {
        let page = tx.read_page::<EntryId>(&record.queue, position, PAGE_SIZE as usize)?;
        position += page.items.len() as u64;
        pending.extend(page.items.iter().map(|item| **item));
      }
      let source =
        pending.iter().position(|id| *id == entry).ok_or(SessionError::InvalidEntry(entry))?;
      if let Some(target) = before {
        if !pending.contains(&target) {
          return Err(SessionError::InvalidEntry(target));
        }
      }
      if before == Some(entry) {
        return Ok(());
      }
      let original = pending.clone();
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
          tx.set_item(&record.queue, record.queue_head + offset as u64, id)?;
        }
      }
      SessionTransaction { record: &mut record, tx, key: &key, recorded_at }
        .record_event(SessionEvent::InputMoved { entry, before })
    })
  }
  /// Atomically remove a pending input, including while a model or tool is running.
  /// A consumed entry cannot be removed; its original content always remains stored.
  pub fn cancel_queued_input(&self, entry: EntryId) -> Result<(), SessionError> {
    let key = self.key.clone();
    let recorded_at = crate::session::statistics::Timestamp::now();
    self.storage.transaction(move |tx| {
      let stored = tx.load_object::<SessionRecord>(&key)?;
      let mut record = (*stored).clone();
      let mut position = record.queue_head;
      let end = tx.list_len::<EntryId>(&record.queue)?;
      while position < end {
        let page = tx.read_page::<EntryId>(&record.queue, position, PAGE_SIZE as usize)?;
        for (offset, item) in page.items.iter().enumerate() {
          if **item == entry {
            tx.remove_item::<EntryId>(&record.queue, position + offset as u64)?;
            return SessionTransaction { record: &mut record, tx, key: &key, recorded_at }
              .record_event(SessionEvent::InputCancelled { entry });
          }
        }
        position += page.items.len() as u64;
      }
      Err(SessionError::InvalidEntry(entry))
    })
  }
}
impl Session {
  pub fn cancel_queued_input(&mut self, entry: EntryId) -> Result<(), SessionError> {
    self.create_sender().cancel_queued_input(entry)
  }
}
