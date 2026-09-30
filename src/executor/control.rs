//! Cancellation of executor work.
use std::{future::Future, sync::Arc};
use tokio::sync::watch;

/// Sticky cancellation for executor work. A session run works on a child control of its own, which
/// the caller's cancellation reaches and dropping the run cancels.
#[derive(Clone)]
pub struct ExecutionControl {
  cancelled: watch::Sender<bool>,
  /// The control this one is a child of: cancelling it cancels this one too.
  parent: Option<Arc<ExecutionControl>>,
}
impl std::fmt::Debug for ExecutionControl {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ExecutionControl").field("cancelled", &self.is_cancelled()).finish()
  }
}
impl Default for ExecutionControl {
  fn default() -> Self {
    Self { cancelled: watch::channel(false).0, parent: None }
  }
}
impl ExecutionControl {
  pub fn new() -> Self {
    Self::default()
  }
  /// A control cancelled with this one, which can also be cancelled on its own.
  pub(crate) fn child(&self) -> Self {
    Self { parent: Some(Arc::new(self.clone())), ..Self::default() }
  }
  /// A guard that cancels this control when dropped, so work scoped to it ends even when the future
  /// driving that work is dropped first.
  pub(crate) fn cancel_on_drop(&self) -> CancelOnDrop {
    CancelOnDrop(self.clone())
  }
  pub fn cancel(&self) {
    self.cancelled.send_replace(true);
  }
  pub fn is_cancelled(&self) -> bool {
    *self.cancelled.borrow() || self.parent.as_ref().is_some_and(|parent| parent.is_cancelled())
  }
  pub async fn wait_for_cancellation(&self) {
    if self.is_cancelled() {
      return;
    }
    let mut receiver = self.cancelled.subscribe();
    let own = receiver.wait_for(|cancelled| *cancelled);
    match &self.parent {
      None => {
        let _ = own.await;
      }
      Some(parent) => {
        tokio::select! {
          _ = own => {}
          () = Box::pin(parent.wait_for_cancellation()) => {}
        }
      }
    }
  }
  /// Drives `work` until it finishes or this control is cancelled, whichever comes first: `None`
  /// when cancelled, with `work` dropped unfinished. A cancellation already signalled wins over
  /// work that is ready too. Use it only for work that is safe to abandon.
  pub async fn run_until_cancelled<F: Future>(&self, work: F) -> Option<F::Output> {
    tokio::select! {
      biased;
      () = self.wait_for_cancellation() => None,
      output = work => Some(output),
    }
  }
}

/// Cancels its control when dropped.
pub(crate) struct CancelOnDrop(ExecutionControl);
impl Drop for CancelOnDrop {
  fn drop(&mut self) {
    self.0.cancel();
  }
}
