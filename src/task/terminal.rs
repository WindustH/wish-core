//! A user-owned PTY, separate from model tasks and their completion notifications.
use super::{TaskConfig, TaskError};
use base64::{Engine, engine::general_purpose::STANDARD};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::{
  collections::VecDeque,
  io::{self, Read, Write},
  sync::{Arc, LazyLock, Mutex},
  time::Duration,
};
use tokio::sync::{Semaphore, oneshot, watch};

const RETAINED: usize = 1024 * 1024;
static SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(16)));

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum TerminalRequest {
  State,
  Start { cols: u16, rows: u16 },
  Poll { id: String, offset: u64 },
  Write { id: String, data: String },
  Resize { id: String, cols: u16, rows: u16 },
  Close { id: String },
}
#[derive(Clone, Serialize)]
pub struct TerminalSnapshot {
  pub id: String,
  pub running: bool,
  pub exit_code: Option<u32>,
  pub error: Option<String>,
  pub cols: u16,
  pub rows: u16,
  pub cwd: String,
}
#[derive(Serialize)]
pub struct TerminalOutput {
  pub terminal: TerminalSnapshot,
  pub data: String,
  pub next_offset: u64,
  pub reset: bool,
  pub eof: bool,
}
struct Buffer {
  bytes: VecDeque<u8>,
  end: u64,
  eof: bool,
}
type Input = (Vec<u8>, oneshot::Sender<io::Result<()>>);
pub(super) struct Terminal {
  state: Arc<Mutex<TerminalSnapshot>>,
  buffer: Arc<Mutex<Buffer>>,
  changed: watch::Sender<u64>,
  master: Mutex<Box<dyn MasterPty + Send>>,
  killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
  input: std::sync::mpsc::SyncSender<Input>,
  #[cfg(unix)]
  pid: Option<u32>,
}
fn io_error(error: impl std::fmt::Display) -> TaskError {
  io::Error::other(error.to_string()).into()
}
fn size(cols: u16, rows: u16) -> Result<PtySize, TaskError> {
  if !(2..=500).contains(&cols) || !(2..=300).contains(&rows) {
    return Err(TaskError::InvalidArguments(
      "terminal size must be 2–500 columns and 2–300 rows".into(),
    ));
  }
  Ok(PtySize { cols, rows, pixel_width: 0, pixel_height: 0 })
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
  value.lock().unwrap_or_else(|error| error.into_inner())
}
fn notify(changed: &watch::Sender<u64>) {
  changed.send_modify(|revision| *revision = revision.wrapping_add(1));
}
impl Terminal {
  // Called on a blocking worker while the registry serializes start against shutdown.
  pub fn start(config: &TaskConfig, cols: u16, rows: u16) -> Result<Arc<Self>, TaskError> {
    let dimensions = size(cols, rows)?;
    let permit = SLOTS
      .clone()
      .try_acquire_owned()
      .map_err(|_| TaskError::InvalidArguments("too many open terminals (maximum 16)".into()))?;
    let shell = config.command.read().map_err(io_error)?.clone();
    let cwd = config.cwd.read().map_err(io_error)?.clone();
    let mut command = CommandBuilder::new(&shell.program);
    // Task arguments contain command-text flags (-c, -Command); interactive shells need their own mode.
    let family =
      shell.program.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_ascii_lowercase();
    match family.as_str() {
      "bash" | "zsh" => command.arg("-l"),
      "sh" | "dash" | "fish" => command.arg("-i"),
      "powershell" | "pwsh" => command.arg("-NoLogo"),
      "cmd" => command.arg("/D"),
      _ => (),
    }
    command.cwd(&cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    for (key, value) in &shell.env {
      if let Some(value) = value {
        command.env(key, value);
      } else {
        command.env_remove(key);
      }
    }
    for (key, value) in &config.env {
      command.env(key, value);
    }
    for (key, value) in config.live_env.read().map_err(io_error)?.iter() {
      command.env(key, value);
    }
    command.env_remove("WISH_TASK_ID");
    let pair = portable_pty::native_pty_system().openpty(dimensions).map_err(io_error)?;
    let mut reader = pair.master.try_clone_reader().map_err(io_error)?;
    let mut writer = pair.master.take_writer().map_err(io_error)?;
    let child = pair.slave.spawn_command(command).map_err(io_error)?;
    drop(pair.slave);
    let killer = child.clone_killer();
    #[cfg(unix)]
    let pid = child.process_id();
    let child = Arc::new(Mutex::new(child));
    let state = Arc::new(Mutex::new(TerminalSnapshot {
      id: uuid::Uuid::new_v4().to_string(),
      running: true,
      exit_code: None,
      error: None,
      cols,
      rows,
      cwd: cwd.to_string_lossy().into_owned(),
    }));
    let buffer = Arc::new(Mutex::new(Buffer { bytes: VecDeque::new(), end: 0, eof: false }));
    let (changed, _) = watch::channel(0);
    let (input, receiver) = std::sync::mpsc::sync_channel::<Input>(8);
    let terminal = Arc::new(Self {
      state: state.clone(),
      buffer: buffer.clone(),
      changed: changed.clone(),
      master: Mutex::new(pair.master),
      killer: Mutex::new(killer),
      input,
      #[cfg(unix)]
      pid,
    });
    let start_workers = || -> io::Result<()> {
      let read_changed = changed.clone();
      let read_state = state.clone();
      std::thread::Builder::new().name("wish-pty-output".into()).spawn(move || {
        let mut chunk = [0; 16384];
        loop {
          match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
              let mut output = lock(&buffer);
              output.end += count as u64;
              let remove = (output.bytes.len() + count).saturating_sub(RETAINED);
              output.bytes.drain(..remove);
              output.bytes.extend(&chunk[..count]);
              drop(output);
              notify(&read_changed);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            // Linux returns EIO after the last slave descriptor closes.
            Err(error) => {
              if error.raw_os_error() != Some(5) {
                lock(&read_state).error = Some(error.to_string());
              }
              break;
            }
          }
        }
        lock(&buffer).eof = true;
        notify(&read_changed);
      })?;
      let input_state = state.clone();
      std::thread::Builder::new().name("wish-pty-input".into()).spawn(move || {
        while lock(&input_state).running {
          let (bytes, reply) = match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(input) => input,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
          };
          let result = writer.write_all(&bytes).and_then(|_| writer.flush());
          let failed = result.is_err();
          let _ = reply.send(result);
          if failed {
            break;
          }
        }
      })?;
      let waiting_child = child.clone();
      std::thread::Builder::new().name("wish-pty-wait".into()).spawn(move || {
        let result = lock(&waiting_child).wait();
        #[cfg(target_os = "linux")]
        if let Some(pid) = pid {
          kill_session_jobs(pid);
        }
        let mut snapshot = lock(&state);
        snapshot.running = false;
        match result {
          Ok(exit) => snapshot.exit_code = Some(exit.exit_code()),
          Err(error) => snapshot.error = Some(error.to_string()),
        }
        drop(snapshot);
        notify(&changed);
        drop(permit);
      })?;
      Ok(())
    };
    if let Err(error) = start_workers() {
      terminal.kill();
      let _ = lock(&child).wait();
      return Err(error.into());
    }
    Ok(terminal)
  }
  pub fn snapshot(&self) -> TerminalSnapshot {
    lock(&self.state).clone()
  }
  pub fn matches(&self, id: &str) -> bool {
    lock(&self.state).id == id
  }
  pub fn running(&self) -> bool {
    lock(&self.state).running
  }
  pub fn resize(&self, cols: u16, rows: u16) -> Result<(), TaskError> {
    lock(&self.master).resize(size(cols, rows)?).map_err(io_error)?;
    let mut state = lock(&self.state);
    state.cols = cols;
    state.rows = rows;
    Ok(())
  }
  pub async fn write(&self, data: String) -> Result<(), TaskError> {
    if !self.running() {
      return Err(TaskError::Closed);
    }
    if data.len() > 44000 {
      return Err(TaskError::InvalidArguments("terminal input exceeds 32 KiB".into()));
    }
    let bytes = STANDARD
      .decode(data)
      .map_err(|_| TaskError::InvalidArguments("invalid base64 terminal input".into()))?;
    if bytes.len() > 32768 {
      return Err(TaskError::InvalidArguments("terminal input exceeds 32 KiB".into()));
    }
    let (reply, received) = oneshot::channel();
    self
      .input
      .try_send((bytes, reply))
      .map_err(|_| TaskError::InvalidArguments("terminal input is busy".into()))?;
    match tokio::time::timeout(Duration::from_secs(5), received).await {
      Ok(Ok(result)) => result.map_err(Into::into),
      _ => {
        self.kill();
        Err(TaskError::Unknown(
          "terminal input stalled; terminal stopped, do not repeat input automatically".into(),
        ))
      }
    }
  }
  pub async fn poll(&self, offset: u64) -> TerminalOutput {
    let mut changes = self.changed.subscribe();
    let wait = {
      let buffer = lock(&self.buffer);
      buffer.end == offset && !(buffer.eof && !self.running())
    };
    if wait {
      let _ = tokio::time::timeout(Duration::from_secs(15), changes.changed()).await;
    }
    let buffer = lock(&self.buffer);
    let start = buffer.end - buffer.bytes.len() as u64;
    let reset = offset < start || offset > buffer.end;
    let offset = if reset { start } else { offset };
    let bytes: Vec<_> =
      buffer.bytes.iter().skip((offset - start) as usize).take(65536).copied().collect();
    TerminalOutput {
      terminal: self.snapshot(),
      next_offset: offset + bytes.len() as u64,
      data: STANDARD.encode(bytes),
      reset,
      eof: buffer.eof,
    }
  }
  pub fn kill(&self) {
    if !self.running() {
      return;
    }
    #[cfg(target_os = "linux")]
    if let Some(pid) = self.pid {
      kill_session_jobs(pid);
    }
    // Interactive shells give foreground jobs their own process group. Signal that group
    // before the shell so a full-screen program cannot retain the slave indefinitely.
    #[cfg(unix)]
    if let Some(group) = lock(&self.master).process_group_leader().filter(|pid| *pid > 1) {
      unsafe {
        libc::kill(-group, libc::SIGKILL);
      }
    }
    #[cfg(unix)]
    if let Some(pid) = self.pid {
      unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
        libc::kill(pid as i32, libc::SIGKILL);
      }
    }
    let _ = lock(&self.killer).kill();
  }
  pub async fn close(&self) {
    let mut changes = self.changed.subscribe();
    self.kill();
    while self.running() {
      if changes.changed().await.is_err() {
        break;
      }
    }
  }
}
impl Drop for Terminal {
  fn drop(&mut self) {
    self.kill();
  }
}

// Linux job-control shells put background jobs in separate process groups. The PTY
// session ID remains reserved while those jobs exist, even after its leader exits.
#[cfg(target_os = "linux")]
fn kill_session_jobs(session: u32) {
  let Ok(entries) = std::fs::read_dir("/proc") else { return };
  for entry in entries.flatten() {
    let Some(pid) =
      entry.file_name().to_str().and_then(|name| name.parse::<i32>().ok()).filter(|pid| *pid > 1)
    else {
      continue;
    };
    if pid == session as i32 {
      continue;
    }
    let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
    let Some((_, fields)) = stat.rsplit_once(") ") else { continue };
    if fields.split_whitespace().nth(3).and_then(|value| value.parse::<u32>().ok()) == Some(session)
    {
      unsafe {
        libc::kill(pid, libc::SIGKILL);
      }
    }
  }
}
