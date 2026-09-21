//! Active/standby context, request construction and context validation.
mod compaction;
mod generation;
mod request;
mod validation;

pub use compaction::{CompactionConfig, CompactionReason};
pub use generation::{Generation, GenerationId, GenerationStatus};
pub(super) use validation::validate_tool_pairs;

use super::{EntryId, EntryOrigin, Session, SessionError, SessionEvent};
use crate::protocol::Message;

impl Session {
  pub fn create_context_entry(&mut self, message: Message) -> Result<EntryId, SessionError> {
    self.update(move |transaction| {
      let entry = transaction.store_entry(message, EntryOrigin::Context)?;
      transaction.record_event(SessionEvent::ContextEntryCreated { entry })?;
      Ok(entry)
    })
  }
}
