use super::edit::{EditCapture, EditResult};
use super::{KillMode, ShellError, platform::ProcessTree};
use crate::executor::ExecutionControl;
use serde::Serialize;
use std::{io, path::PathBuf, sync::Arc, time::Duration};
use tokio::{
  io::AsyncWriteExt,
  process::{Child, ChildStdin},
  sync::{Mutex, mpsc, watch},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
  Running,
  Exited,
  Killed,
  Unknown,
}
#[derive(Clone, Debug, Serialize)]
pub(super) struct Snapshot {
  pub status: Status,
  pub exit_code: Option<i32>,
  pub term_signal: Option<i32>,
  pub error: Option<String>,
  pub initial_input_bytes: usize,
  pub initial_input_error: Option<String>,
  #[serde(skip)]
  pub edits: Arc<Vec<EditResult>>,
}
impl Default for Snapshot {
  fn default() -> Self {
    Self {
      status: Status::Running,
      exit_code: None,
      term_signal: None,
      error: None,
      initial_input_bytes: 0,
      initial_input_error: None,
      edits: Arc::default(),
    }
  }
}
pub(super) struct Execution {
  pub id: String,
  pub output_path: PathBuf,
  pub stdin: Mutex<Option<ChildStdin>>,
  pub state: watch::Sender<Snapshot>,
  pub kills: mpsc::UnboundedSender<KillMode>,
  pub write_timeout: Duration,
  pub initial_input_done: watch::Sender<bool>,
}
impl Execution {
  pub fn get_snapshot(&self) -> Snapshot {
    self.state.borrow().clone()
  }
  pub async fn wait_for_exit(&self) -> Result<Snapshot, ShellError> {
    let mut state = self.state.subscribe();
    loop {
      let snapshot = state.borrow_and_update().clone();
      if snapshot.status != Status::Running {
        if snapshot.status == Status::Unknown {
          return Err(ShellError::Unknown(snapshot.error.unwrap_or_default()));
        }
        return Ok(snapshot);
      }
      state
        .changed()
        .await
        .map_err(|_| ShellError::Unknown("process supervisor stopped".into()))?;
    }
  }
  pub async fn terminate(&self, mode: KillMode) -> Result<Snapshot, ShellError> {
    if self.get_snapshot().status == Status::Running {
      // A concurrent natural exit may have closed the mailbox; the retained state is authoritative.
      if self.kills.send(mode).is_err() && self.get_snapshot().status == Status::Running {
        return Err(ShellError::Unknown("process supervisor stopped".into()));
      }
    }
    self.wait_for_exit().await
  }
  pub async fn write_bytes(
    &self,
    bytes: &[u8],
    close: bool,
    control: &ExecutionControl,
    wait_for_initial: bool,
  ) -> WriteResult {
    let mut accepted = 0;
    let mut error = None;
    let mut available = true;
    let writing = async {
      if wait_for_initial {
        let mut initial = self.initial_input_done.subscribe();
        initial
          .wait_for(|done| *done)
          .await
          .map_err(|_| io::Error::other("initial stdin writer stopped"))?;
      }
      let mut stdin = self.stdin.lock().await;
      let Some(pipe) = stdin.as_mut() else {
        available = false;
        return Err(io::Error::new(io::ErrorKind::BrokenPipe, "stdin is closed"));
      };
      while accepted < bytes.len() {
        let written = pipe.write(&bytes[accepted..]).await?;
        if written == 0 {
          return Err(io::Error::new(io::ErrorKind::WriteZero, "stdin accepted no bytes"));
        }
        accepted += written;
      }
      if close {
        stdin.take();
        available = false;
      }
      Ok(())
    };
    let mut interrupted = false;
    let mut timed_out = false;
    tokio::select! {
      biased;
      _ = control.wait_for_cancellation() => interrupted = true,
      result = tokio::time::timeout(self.write_timeout, writing) => match result {
        Ok(Err(failure)) => error = Some(failure.to_string()),
        Err(_) => timed_out = true,
        Ok(Ok(())) => {},
      }
    }
    WriteResult {
      accepted_bytes: accepted,
      stdin_available: available,
      interrupted,
      timed_out,
      error,
    }
  }
}
#[derive(Serialize)]
pub(super) struct WriteResult {
  pub accepted_bytes: usize,
  pub stdin_available: bool,
  pub interrupted: bool,
  pub timed_out: bool,
  pub error: Option<String>,
}

pub(super) async fn supervise(
  mut child: Child,
  mut group: ProcessTree,
  execution: Arc<Execution>,
  mut kills: mpsc::UnboundedReceiver<KillMode>,
  grace: Duration,
  initial_input: Option<tokio::task::JoinHandle<()>>,
  edits: Vec<EditCapture>,
) {
  let mut killed = false;
  let mut deadline = None;
  let result = loop {
    tokio::select! {
      result = child.wait() => break result,
      Some(mode) = kills.recv() => {
        killed = true;
        let force = matches!(mode, KillMode::Force);
        if let Err(error) = group.signal(force) { break Err(error); }
        if force { deadline = None; }
        else if deadline.is_none() { deadline = tokio::time::Instant::now().checked_add(grace); }
      }
      _ = async {
        if let Some(deadline) = deadline { tokio::time::sleep_until(deadline).await; }
        else { std::future::pending::<()>().await; }
      } => {
        if let Err(error) = group.signal(true) { break Err(error); }
        deadline = None;
      }
    }
  };
  // A shell may exit while ordinary descendants still hold its output or stdin. Do not leave
  // these running after reporting terminal completion. Explicit start soft-timeout never gets here.
  let cleanup = group.signal(true);
  if cleanup.is_ok() {
    group.disarm();
  }
  if result.is_err() {
    let _ = child.kill().await;
  }
  if let Some(initial_input) = initial_input {
    let _ = initial_input.await;
  }
  execution.stdin.lock().await.take();
  let edits = futures_util::future::join_all(edits.into_iter().map(EditCapture::finish)).await;
  execution.state.send_modify(|snapshot| {
    snapshot.edits = Arc::new(edits);
    match result.and_then(|status| cleanup.map(|_| status)) {
      Ok(status) => {
        snapshot.status = if killed { Status::Killed } else { Status::Exited };
        snapshot.exit_code = status.code();
        #[cfg(unix)]
        {
          use std::os::unix::process::ExitStatusExt;
          snapshot.term_signal = status.signal();
        }
      }
      Err(error) => {
        snapshot.status = Status::Unknown;
        snapshot.error = Some(error.to_string());
      }
    }
  });
}
