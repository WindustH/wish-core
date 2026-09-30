//! Optional per-attempt observations; implementations must keep callbacks nonblocking.
use crate::protocol::StreamEvent;
use std::sync::Arc;

/// Constructed just before a physical streaming attempt, retries and streamed compaction included.
/// Dropped at terminal, error or abort. Events are observed once when decoded, independently of
/// what the application reads or replays.
pub trait AttemptObserver: Send + Sync {
  fn observe(&self, event: &StreamEvent);
}
/// Makes the observer of one attempt, given the model it calls.
pub type AttemptObserverFactory = Arc<dyn Fn(&str) -> Box<dyn AttemptObserver> + Send + Sync>;
