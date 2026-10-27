//! Input arrivals visible to tools running on behalf of a session.
use super::{Session, SessionError, SessionReader};
use tokio::sync::watch;
/// An event subscription installed before checking the durable queue. No timer or polling.
/// Background completions wake idle sessions, but do not preempt unrelated foreground work.
#[derive(Clone)]
pub struct ToolInput {
  reader: SessionReader,
  head: u64,
  arrivals: watch::Receiver<()>,
}
impl ToolInput {
  pub(crate) fn new(session: &Session) -> Self {
    let arrivals = session.watch_input_arrivals();
    Self { reader: session.reader().clone(), head: session.get_queue_head(), arrivals }
  }
  pub fn pending(&self) -> Result<bool, SessionError> {
    for page in self.reader.get_message_queue().pages(self.head) {
      for id in page?.items {
        if let Some(entry) = self.reader.get_entry(*id)? {
          let source = entry.message.get_metadata()["source"].as_str().unwrap_or_default();
          if !matches!(source, "background_execution_finished" | "background_tool_finished") {
            return Ok(true);
          }
        }
      }
    }
    Ok(false)
  }
  pub async fn wait(&mut self) -> Result<(), SessionError> {
    loop {
      let snapshot = self.clone();
      if crate::utils::blocking::io(move || snapshot.pending()).await.map_err(|error| {
        super::SessionError::Storage(crate::storage::StorageError::Corrupt(format!(
          "input reader failed: {error}"
        )))
      })?? {
        return Ok(());
      }
      // A closed session will never receive new input. It is cancelled by its owner instead.
      if self.arrivals.changed().await.is_err() {
        std::future::pending::<()>().await;
      }
    }
  }
}
