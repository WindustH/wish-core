//! Closing Wish's console window or shutting Windows down, as a graceful shutdown.
//!
//! Windows ends the process as soon as the handler for these events returns, so the handler here
//! holds the event until the server has shut down, within the seconds Windows allows. Logoff is
//! left alone: only a service's console receives it, whenever any user logs off, and a service
//! is meant to ignore it.
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::Notify;
use windows_sys::Win32::System::Console::{
  CTRL_CLOSE_EVENT, CTRL_SHUTDOWN_EVENT, SetConsoleCtrlHandler,
};

/// Windows allows about five seconds after a close event before it ends the process anyway.
const HOLD: Duration = Duration::from_millis(4500);

struct Ending {
  requested: Notify,
  finished: Mutex<bool>,
  released: Condvar,
}

static ENDING: OnceLock<Ending> = OnceLock::new();

fn ending() -> &'static Ending {
  ENDING.get_or_init(|| Ending {
    requested: Notify::new(),
    finished: Mutex::new(false),
    released: Condvar::new(),
  })
}

unsafe extern "system" fn handle(event: u32) -> i32 {
  if !matches!(event, CTRL_CLOSE_EVENT | CTRL_SHUTDOWN_EVENT) {
    return 0;
  }
  let ending = ending();
  ending.requested.notify_one();
  let finished = ending.finished.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  let _ = ending.released.wait_timeout_while(finished, HOLD, |finished| !*finished);
  1
}

pub(super) fn install() {
  ending();
  // SAFETY: `handle` is a valid handler for the life of the process and touches only statics.
  unsafe {
    SetConsoleCtrlHandler(Some(handle), 1);
  }
}

/// Resolves once the console is closing or Windows is shutting down.
pub(super) async fn wait_for_close() {
  ending().requested.notified().await;
}

/// Lets a held close or shutdown event end the process now that the server has shut down.
pub(super) fn release() {
  let ending = ending();
  *ending.finished.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
  ending.released.notify_all();
}
