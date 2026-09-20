use crate::session::{HistoryItem, Session, SessionError, SessionEvent};

pub(super) fn notify_observers(
  session: &Session,
  cursor: &mut u64,
  observe: &mut impl FnMut(&SessionEvent),
) -> Result<(), SessionError> {
  let history = session.get_history();
  let end = history.len()?;
  while *cursor < end {
    let page = history.read_page(*cursor, crate::storage::PAGE_SIZE as usize)?;
    for record in &page.items {
      if let HistoryItem::Event(id) = record.item {
        let event = session
          .get_event(id)?
          .ok_or_else(|| crate::storage::StorageError::Corrupt("missing event".into()))?;
        // Stream events were already delivered live by receive_stream.
        if !matches!(&*event, SessionEvent::ModelStream(_)) {
          observe(&event);
        }
      }
    }
    *cursor += page.items.len() as u64;
  }
  Ok(())
}
