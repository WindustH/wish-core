use tokio::sync::watch;

/// Sticky, cloneable cancellation shared by a run and its tool executor.
#[derive(Clone, Debug)]
pub struct RunControl(watch::Sender<bool>);

impl Default for RunControl {
  fn default() -> Self {
    Self(watch::channel(false).0)
  }
}

impl RunControl {
  pub fn new() -> Self {
    Self::default()
  }
  pub fn cancel(&self) {
    self.0.send_replace(true);
  }
  pub fn is_cancelled(&self) -> bool {
    *self.0.borrow()
  }
  pub async fn wait_for_cancellation(&self) {
    let mut receiver = self.0.subscribe();
    let _ = receiver.wait_for(|cancelled| *cancelled).await;
  }
}
