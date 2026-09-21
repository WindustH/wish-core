//! Optional per-attempt observations; implementations must keep callbacks nonblocking.
use crate::protocol::StreamEvent;
use std::sync::Arc;

/// Constructed just before a physical streaming attempt. Dropped at terminal, error or abort.
/// Events are observed once when decoded, independently of application reads/replay.
pub trait StreamObserver: Send + Sync {
  fn observe(&self, event: &StreamEvent);
}
pub type StreamObserverFactory = Arc<dyn Fn(&str) -> Box<dyn StreamObserver> + Send + Sync>;
