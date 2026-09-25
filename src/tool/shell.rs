//! Stateful shell tool: start, poll, write and kill. One instance owns its executions.
mod edit;
mod operation;
mod platform;
mod process;
mod runtime;

pub use operation::{DataEncoding, KillMode, ShellOperation};
pub use runtime::ShellTool;
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
pub struct ShellConfig {
  /// Read at each start, so replacing it reaches the next command of every tool sharing it.
  pub command: Arc<RwLock<ShellCommand>>,
  pub cwd: PathBuf,
  pub capture_dir: PathBuf,
  /// Overrides on top of the application's inherited environment.
  pub env: BTreeMap<String, String>,
  pub soft_timeout: Duration,
  pub inline_bytes: usize,
  pub poll_bytes: usize,
  pub kill_grace: Duration,
  pub stdin_write_timeout: Duration,
}
impl ShellConfig {
  pub fn new(cwd: impl Into<PathBuf>, capture_dir: impl Into<PathBuf>) -> Self {
    Self {
      command: Arc::new(RwLock::new(ShellCommand::platform_default())),
      cwd: cwd.into(),
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
pub enum ShellError {
  #[error("shell operation interrupted")]
  Interrupted,
  #[error("invalid shell arguments: {0}")]
  InvalidArguments(String),
  #[error("unknown shell execution: {0}")]
  ExecutionNotFound(String),
  #[error("shell runtime is shut down")]
  Closed,
  #[error("shell execution outcome is unknown: {0}")]
  Unknown(String),
  #[error(transparent)]
  Io(#[from] std::io::Error),
}
