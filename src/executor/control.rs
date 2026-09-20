use std::sync::Arc;
use tokio::sync::watch;

/// Sticky cancellation for executor work. A session run creates its own scoped child control.
#[derive(Clone)]
pub struct ExecutionControl {
  cancelled: watch::Sender<bool>,
  inherited: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}
impl std::fmt::Debug for ExecutionControl {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("ExecutionControl").field("cancelled", &self.is_cancelled()).finish()
  }
}
impl Default for ExecutionControl {
  fn default() -> Self {
    Self { cancelled: watch::channel(false).0, inherited: None }
  }
}
impl ExecutionControl {
  pub fn new() -> Self {
    Self::default()
  }
  pub(crate) fn inherit(check: impl Fn() -> bool + Send + Sync + 'static) -> Self {
    Self { inherited: Some(Arc::new(check)), ..Self::default() }
  }
  pub(crate) fn create_scope(&self) -> ExecutionScope {
    ExecutionScope(self.clone())
  }
  pub fn cancel(&self) {
    self.cancelled.send_replace(true);
  }
  pub fn is_cancelled(&self) -> bool {
    *self.cancelled.borrow() || self.inherited.as_ref().is_some_and(|check| check())
  }
  pub async fn wait_for_cancellation(&self) {
    if self.is_cancelled() {
      return;
    }
    let mut receiver = self.cancelled.subscribe();
    let _ = receiver.wait_for(|cancelled| *cancelled).await;
  }
}

/// End the child scope even when the run future is dropped before returning.
pub(crate) struct ExecutionScope(ExecutionControl);
impl Drop for ExecutionScope {
  fn drop(&mut self) {
    self.0.cancel();
  }
}
