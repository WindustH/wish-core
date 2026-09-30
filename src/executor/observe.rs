//! Delivery of committed session events to a run's observer.
use crate::session::{HistoryItem, Session, SessionError, SessionEvent};
use crate::storage::StorageError;
use crate::utils::time::Timestamp;

/// Delivers every event committed to the session's history from position `delivered` on, in order,
/// and moves `delivered` past them. Stream events are left out: they reached the observer live, as
/// they arrived.
pub(crate) fn deliver_new_events(
  session: &Session,
  delivered: &mut u64,
  observe: &mut impl FnMut(&SessionEvent),
) -> Result<(), SessionError> {
  let reader = session.reader();
  for page in reader.get_history().pages(*delivered) {
    let page = page?;
    for record in &page.items {
      if let HistoryItem::Event(id) = record.item {
        let event =
          reader.get_event(id)?.ok_or_else(|| StorageError::Corrupt("missing event".into()))?;
        if !matches!(&*event, SessionEvent::ModelStream(_)) {
          observe(&event);
        }
      }
    }
    *delivered = page.end;
  }
  Ok(())
}

/// Records `event` now, then delivers it with anything committed before it.
pub(crate) fn record_and_deliver(
  session: &mut Session,
  delivered: &mut u64,
  observe: &mut impl FnMut(&SessionEvent),
  event: SessionEvent,
) -> Result<(), SessionError> {
  session.record_events(vec![(Timestamp::now(), event)])?;
  deliver_new_events(session, delivered, observe)
}
