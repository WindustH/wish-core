use super::{EntryId, SessionError, SessionRecord, edit::SessionEdit};
use crate::{protocol::Message, storage::Storage};

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
      SessionEdit { record: &mut record, tx, key: &key, recorded_at }.append_input(message)
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
