//! Session-scoped control intentions. Execution cancellation is owned by the executor.
use super::{EntryId, SessionError, SessionSender};
use crate::protocol::Message;
use tokio::sync::watch;

/// Control the live Session owner without borrowing it from the executor.
/// A handle belongs to this owner; loading a new Session requires creating a new handle.
#[derive(Clone)]
pub struct SessionHandle {
  pub(super) sender: SessionSender,
  pub(super) control: SessionControl,
}
impl SessionHandle {
  pub fn enqueue_message(&self, message: Message) -> Result<EntryId, SessionError> {
    self.sender.enqueue_message(message)
  }
  /// Request interruption of the current run. False means there is no active run.
  /// True acknowledges the request, not completion; await the executor to finish cleanup.
  pub fn interrupt(&self) -> bool {
    self.control.interrupt()
  }
}

#[derive(Clone)]
pub(super) struct SessionControl(watch::Sender<Option<bool>>);
impl Default for SessionControl {
  fn default() -> Self {
    Self(watch::channel(None).0)
  }
}
impl SessionControl {
  pub fn interrupt(&self) -> bool {
    let mut active = false;
    self.0.send_if_modified(|state| {
      if let Some(interrupted) = state {
        active = true;
        if !*interrupted {
          *interrupted = true;
          return true;
        }
      }
      false
    });
    active
  }
  pub fn begin_run(&self) -> RunRegistration {
    self.0.send_replace(Some(false));
    RunRegistration(self.0.clone())
  }
}

/// Deregister on every exit, including errors and a dropped executor future.
pub(crate) struct RunRegistration(watch::Sender<Option<bool>>);
impl RunRegistration {
  pub fn create_interruption_check(&self) -> impl Fn() -> bool + Send + Sync + 'static {
    let state = self.0.clone();
    move || *state.borrow() == Some(true)
  }

  pub async fn wait_for_interruption(&self) {
    let mut receiver = self.0.subscribe();
    let _ = receiver.wait_for(|state| *state == Some(true)).await;
  }
}
impl Drop for RunRegistration {
  fn drop(&mut self) {
    self.0.send_replace(None);
  }
}
