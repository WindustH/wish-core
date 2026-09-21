//! Stateful shell tool: start, poll, write and kill. One instance owns its executions.
mod operation;
mod platform;
mod process;
mod runtime;

pub use operation::{DataEncoding, KillMode, ShellOperation};
pub use runtime::ShellTool;
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

#[derive(Clone, Debug)]
pub struct ShellConfig {
  pub program: PathBuf,
  /// Arguments before the command text, which is passed as one final argument.
  pub args: Vec<String>,
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
      program: platform::default_program(),
      args: platform::default_args(),
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
