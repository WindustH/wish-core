//! Supervised session tasks: process lifecycle, output, stdin and file-edit capture.
mod edit;
mod operation;
mod platform;
mod process;
mod runtime;

pub use operation::{DataEncoding, KillMode, TaskOperation};
pub use runtime::TaskManager;
use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::{Arc, RwLock},
  time::Duration,
};

/// The program a command runs under and the arguments placed before the command text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellCommand {
  pub program: PathBuf,
  /// Arguments before the command text, which is passed as one final argument.
  pub args: Vec<String>,
}
impl ShellCommand {
  /// `/bin/sh -c` on Unix, `%COMSPEC% /D /S /C` on Windows.
  pub fn platform_default() -> Self {
    Self { program: platform::default_program(), args: platform::default_args() }
  }
  /// The shell at `program` with the arguments its family takes before command text.
  pub fn for_program(program: PathBuf) -> Self {
    let args = platform::family_args(&program);
    Self { program, args }
  }
  /// Shells found on `PATH`, one per known name, in a fixed order.
  pub fn installed() -> Vec<Self> {
    platform::installed_shells().into_iter().map(Self::for_program).collect()
  }
}
/// Whether `path` is a file this process may execute.
pub fn is_runnable(path: &Path) -> bool {
  platform::is_executable_file(path)
}

#[derive(Clone, Debug)]
pub struct TaskConfig {
  /// Read at each start, so replacing it reaches the next command of every tool sharing it.
  pub command: Arc<RwLock<ShellCommand>>,
  pub cwd: Arc<RwLock<PathBuf>>,
  pub capture_dir: PathBuf,
  /// Overrides on top of the application's inherited environment.
  pub env: BTreeMap<String, String>,
  pub soft_timeout: Duration,
  pub inline_bytes: usize,
  pub poll_bytes: usize,
  pub kill_grace: Duration,
  pub stdin_write_timeout: Duration,
}
impl TaskConfig {
  pub fn new(cwd: impl Into<PathBuf>, capture_dir: impl Into<PathBuf>) -> Self {
    Self {
      command: Arc::new(RwLock::new(ShellCommand::platform_default())),
      cwd: Arc::new(RwLock::new(cwd.into())),
      capture_dir: capture_dir.into(),
      env: BTreeMap::new(),
      soft_timeout: Duration::from_secs(10),
      inline_bytes: 64 * 1024,
      poll_bytes: 64 * 1024,
      kill_grace: Duration::from_secs(1),
      stdin_write_timeout: Duration::from_secs(10),
    }
  }
}

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
  #[error("task operation interrupted")]
  Interrupted,
  #[error("invalid task arguments: {0}")]
  InvalidArguments(String),
  #[error(
    "unknown task: {0}; use `wish task list` (or task action=list) to find an existing task_id in this session; do not retry the same missing id"
  )]
  ExecutionNotFound(String),
  #[error("task runtime is shut down")]
  Closed,
  #[error("task outcome is unknown: {0}")]
  Unknown(String),
  #[error(transparent)]
  Io(#[from] std::io::Error),
}

pub const SHELL_DESCRIPTION_NOTE: &str = platform::DESCRIPTION_NOTE;

/// Data captured by a foreground command. Presentation is the caller's responsibility.
pub struct ForegroundResult {
  pub output: serde_json::Value,
  pub edits: serde_json::Value,
  pub failed: bool,
}
/// One-shot arrival signal supplied by the owner; the process supervisor has no session dependency.
pub type InputArrival = std::pin::Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

pub use crate::utils::resource_id::is_execution_id;
