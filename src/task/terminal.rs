//! A user-owned PTY, separate from model tasks and their completion notifications.
use super::{TaskConfig, TaskError};
use base64::{Engine, engine::general_purpose::STANDARD};
use portable_pty::{CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
  collections::VecDeque,
  io::{self, Read, Write},
  sync::{Arc, LazyLock, Mutex},
  time::Duration,
};
use tokio::sync::{Semaphore, oneshot, watch};

const RETAINED: usize = 1024 * 1024;
static SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(16)));

/// Settings for user terminals, independent of the model command runner.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalSettings {
  pub shell: String,
  pub persistence: Persistence,
  pub per_session: bool,
  pub session_name: String,
  pub font_size: u16,
  pub colors: std::collections::BTreeMap<String, String>,
}
impl Default for TerminalSettings {
  fn default() -> Self {
    Self {
      shell: String::new(),
      persistence: Persistence::None,
      per_session: false,
      session_name: String::new(),
      font_size: 13,
      colors: Default::default(),
    }
  }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Persistence {
  #[default]
  None,
  Tmux,
  Zellij,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum TerminalRequest {
  State,
  Start {
    cols: u16,
    rows: u16,
  },
  Poll {
    id: String,
    offset: u64,
  },
  Write {
    id: String,
    data: String,
  },
  Resize {
    id: String,
    cols: u16,
    rows: u16,
  },
  Close {
    id: String,
  },
  Leave {
    id: String,
    #[serde(default)]
    force: bool,
  },
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
  pub persistent: bool,
  pub busy: bool,
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
pub struct Terminal {
  state: Arc<Mutex<TerminalSnapshot>>,
  buffer: Arc<Mutex<Buffer>>,
  changed: watch::Sender<u64>,
  master: Mutex<Box<dyn MasterPty + Send>>,
  killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
  input: std::sync::mpsc::SyncSender<Input>,
  connections: std::sync::atomic::AtomicUsize,
  connection_epoch: std::sync::atomic::AtomicU64,
  last_activity: Mutex<std::time::Instant>,
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
  pub fn start(
    config: &TaskConfig,
    settings: &TerminalSettings,
    cols: u16,
    rows: u16,
  ) -> Result<Arc<Self>, TaskError> {
    let dimensions = size(cols, rows)?;
    let permit = SLOTS
      .clone()
      .try_acquire_owned()
      .map_err(|_| TaskError::InvalidArguments("too many open terminals (maximum 16)".into()))?;
    let shell = config.command.read().map_err(io_error)?.clone();
    let cwd = config.cwd.read().map_err(io_error)?.clone();
    // A user terminal follows the host login shell, independently of the model's
    // command runner. portable-pty resolves $SHELL / the account shell and uses
    // login argv[0] on Unix; Windows uses COMSPEC with startup processing enabled.
    let mut native = CommandBuilder::new_default_prog();
    if !settings.shell.trim().is_empty() {
      #[cfg(unix)]
      native.env("SHELL", settings.shell.trim());
      #[cfg(windows)]
      {
        native = CommandBuilder::new(settings.shell.trim());
      }
      // Never silently fall back if the explicitly selected shell is missing.
      if !std::path::Path::new(settings.shell.trim()).is_file() {
        return Err(TaskError::InvalidArguments(
          "terminal.shell must name an installed executable".into(),
        ));
      }
    }
    #[cfg(unix)]
    let selected_shell = native.get_shell();
    #[cfg(not(unix))]
    let selected_shell = if settings.shell.trim().is_empty() {
      native.get_shell()
    } else {
      settings.shell.trim().to_owned()
    };
    if !settings.shell.trim().is_empty() && selected_shell != settings.shell.trim() {
      return Err(TaskError::InvalidArguments(
        "the selected terminal shell is not executable".into(),
      ));
    }
    let login = matches!(
      std::path::Path::new(&selected_shell).file_stem().and_then(|name| name.to_str()),
      Some("sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "csh" | "tcsh" | "nu")
    );
    let name =
      format!("wish-{:x}", Sha256::digest(config.capture_dir.to_string_lossy().as_bytes()));
    let mut command = match settings.persistence {
      Persistence::None => native,
      Persistence::Tmux => {
        let mut cmd = CommandBuilder::new("tmux");
        if settings.per_session {
          // A private server per Wish session isolates config, environment and naming.
          cmd.args(["-L", &name, "new-session", "-A", "-s", &name, "-c"]);
          cmd.arg(&cwd);
          cmd.arg(&selected_shell);
          if login {
            cmd.arg("-l");
          }
        } else {
          cmd.arg("attach-session");
          if !settings.session_name.trim().is_empty() {
            cmd.args(["-t", settings.session_name.trim()]);
          }
        }
        cmd
      }
      Persistence::Zellij => {
        let mut cmd = CommandBuilder::new("zellij");
        cmd.arg("attach");
        if settings.per_session {
          cmd.args(["--create", &name]);
        } else if !settings.session_name.trim().is_empty() {
          cmd.arg(settings.session_name.trim());
        } else {
          cmd.args(["--index", "0"]);
        }
        cmd.args(["options", "--on-force-close", "detach"]);
        if settings.per_session {
          // Zellij starts default_shell without login arguments. Keep a stable,
          // session-local launcher so every new pane loads the selected profile.
          #[cfg(unix)]
          {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir_all(&config.capture_dir)?;
            let launcher = config
              .capture_dir
              .join(format!("terminal-login-{:x}.sh", Sha256::digest(selected_shell.as_bytes())));
            let script = format!(
              "#!/bin/sh\nexec '{}' {} \"$@\"\n",
              selected_shell.replace('\'', "'\"'\"'"),
              if login { "-l" } else { "" }
            );
            if std::fs::read(&launcher).ok().as_deref() != Some(script.as_bytes()) {
              crate::utils::atomic_file::write_sync(&launcher, script.as_bytes())?;
            }
            std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700))?;
            cmd.arg("--default-shell");
            cmd.arg(launcher);
          }
          #[cfg(not(unix))]
          cmd.args(["--default-shell", &selected_shell]);
        }
        cmd
      }
    };
    command.cwd(&cwd);
    // This PTY belongs to the web emulator, even when the server was launched
    // from another terminal. Brand hints can make TUIs skip capability probes.
    for key in [
      "TERM_PROGRAM_VERSION",
      "KITTY_WINDOW_ID",
      "KITTY_PID",
      "KITTY_LISTEN_ON",
      "KONSOLE_VERSION",
      "ITERM_SESSION_ID",
      "WEZTERM_EXECUTABLE",
      "GHOSTTY_RESOURCES_DIR",
      "WT_SESSION",
      "WARP_HONOR_PS1",
      "VSCODE_INJECTION",
      "TABBY_CONFIG_DIRECTORY",
    ] {
      command.env_remove(key);
    }
    command.env("TERM_PROGRAM", "wish");
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
    command.env_remove("TMUX");
    command.env_remove("TMUX_PANE");
    command.env_remove("ZELLIJ");
    command.env_remove("ZELLIJ_SESSION_NAME");
    if !settings.shell.trim().is_empty() {
      command.env("SHELL", &selected_shell);
    }
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
      persistent: settings.persistence != Persistence::None,
      busy: false,
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
      connections: 0.into(),
      connection_epoch: 0.into(),
      last_activity: Mutex::new(std::time::Instant::now()),
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
    let weak = Arc::downgrade(&terminal);
    tokio::spawn(async move {
      loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let Some(terminal) = weak.upgrade() else { break };
        if terminal.connection_epoch.load(std::sync::atomic::Ordering::SeqCst) != 0
          || !terminal.running()
        {
          break;
        }
        // Also permit legacy HTTP polling clients; a lost start/ticket response
        // must not pin a shell and its session cache indefinitely.
        if lock(&terminal.last_activity).elapsed() >= Duration::from_secs(30) {
          terminal.close().await;
          break;
        }
      }
    });
    Ok(terminal)
  }
  pub fn attach(&self) {
    use std::sync::atomic::Ordering::SeqCst;
    self.connections.fetch_add(1, SeqCst);
    self.connection_epoch.fetch_add(1, SeqCst);
  }
  pub fn detach(self: &Arc<Self>) {
    use std::sync::atomic::Ordering::SeqCst;
    if self.connections.fetch_sub(1, SeqCst) != 1 {
      return;
    }
    let epoch = self.connection_epoch.load(SeqCst);
    let weak = Arc::downgrade(self);
    tokio::spawn(async move {
      // Brief network interruptions can reattach without losing an ordinary shell.
      tokio::time::sleep(Duration::from_secs(30)).await;
      if let Some(terminal) = weak.upgrade()
        && terminal.connections.load(SeqCst) == 0
        && terminal.connection_epoch.load(SeqCst) == epoch
      {
        terminal.close().await;
      }
    });
  }
  /// Output preceding this cursor belongs to a newly attached client's replay.
  pub fn output_offset(&self) -> u64 {
    lock(&self.buffer).end
  }
  pub fn snapshot(&self) -> TerminalSnapshot {
    lock(&self.state).clone()
  }
  pub fn has_jobs(&self) -> bool {
    #[cfg(target_os = "linux")]
    if let Some(pid) = self.pid {
      let Ok(entries) = std::fs::read_dir("/proc") else { return true };
      for entry in entries.flatten() {
        if entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) == Some(pid) {
          continue;
        }
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
        let Some((_, fields)) = stat.rsplit_once(") ") else { continue };
        let fields: Vec<_> = fields.split_whitespace().collect();
        if fields.first() != Some(&"Z")
          && fields.get(3).and_then(|s| s.parse::<u32>().ok()) == Some(pid)
        {
          return true;
        }
      }
      // A shell executing a builtin loop is busy even without child processes.
      let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
      return stat.rsplit_once(") ").is_none_or(|(_, fields)| !fields.starts_with("S "));
    }
    // On platforms without a full process-tree probe, err on the side of asking.
    true
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
    *lock(&self.last_activity) = std::time::Instant::now();
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
    // A program may intentionally stop reading stdin. Backpressure is not a
    // reason to kill it; closing the PTY releases this bounded writer instead.
    match received.await {
      Ok(result) => result.map_err(Into::into),
      Err(_) if !self.running() => Err(TaskError::Closed),
      Err(_) => Err(TaskError::Unknown(
        "terminal input writer closed; do not repeat input automatically".into(),
      )),
    }
  }

  pub async fn poll(&self, offset: u64) -> TerminalOutput {
    *lock(&self.last_activity) = std::time::Instant::now();
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
