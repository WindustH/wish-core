//! Typed task operations shared by the CLI bridge and legacy tool adapter.
use super::TaskError;
use base64::Engine;
use serde::{Deserialize, Serialize};

/// One task operation and its arguments.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskOperation {
  Start {
    command: String,
    /// Seconds to wait for completion before returning execution_id while the command continues
    /// in the background. Expiry never terminates the command. -1 or omission uses the configured
    /// default; 0 starts the command and returns without waiting for completion.
    #[serde(default)]
    timeout: Option<f64>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    encoding: DataEncoding,
    #[serde(default)]
    interactive: bool,
  },
  Poll {
    execution_id: String,
    #[serde(default)]
    offset: u64,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    wait_ms: u64,
    #[serde(default)]
    encoding: DataEncoding,
  },
  Write {
    execution_id: String,
    #[serde(default)]
    data: String,
    #[serde(default)]
    encoding: DataEncoding,
    #[serde(default)]
    close: bool,
  },
  Kill {
    execution_id: String,
    #[serde(default)]
    mode: KillMode,
  },
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataEncoding {
  #[default]
  Utf8,
  Base64,
}
impl DataEncoding {
  pub(super) fn decode(self, data: String) -> Result<Vec<u8>, TaskError> {
    match self {
      Self::Utf8 => Ok(data.into_bytes()),
      Self::Base64 => base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| TaskError::InvalidArguments(format!("`data` is not valid base64: {error}; send valid base64, or set encoding to utf8 for plain text"))),
    }
  }
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KillMode {
  #[default]
  Graceful,
  Force,
}
