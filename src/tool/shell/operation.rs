use super::ShellError;
use base64::Engine;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShellOperation {
  Start {
    command: String,
    /// Absolute file path to compare before launch and after the command finishes.
    #[serde(default)]
    edit: Option<std::path::PathBuf>,
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
impl ShellOperation {
  pub fn parse_call(name: &str, mut arguments: serde_json::Value) -> Result<Self, ShellError> {
    match name {
      "shell_start" => {
        if let Some(object) = arguments.as_object_mut() {
          object.insert("operation".into(), "start".into());
        }
        serde_json::from_value(arguments)
          .map_err(|error| ShellError::InvalidArguments(error.to_string()))
      }
      "shell_poll" => {
        if let Some(object) = arguments.as_object_mut() {
          object.insert("operation".into(), "poll".into());
        }
        serde_json::from_value(arguments)
          .map_err(|error| ShellError::InvalidArguments(error.to_string()))
      }
      "shell_write" => {
        if let Some(object) = arguments.as_object_mut() {
          object.insert("operation".into(), "write".into());
        }
        serde_json::from_value(arguments)
          .map_err(|error| ShellError::InvalidArguments(error.to_string()))
      }
      "shell_kill" => {
        if let Some(object) = arguments.as_object_mut() {
          object.insert("operation".into(), "kill".into());
        }
        serde_json::from_value(arguments)
          .map_err(|error| ShellError::InvalidArguments(error.to_string()))
      }
      _ => Err(ShellError::InvalidArguments(format!("unknown shell tool: {name}"))),
    }
  }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataEncoding {
  #[default]
  Utf8,
  Base64,
}
impl DataEncoding {
  pub(super) fn decode(self, data: String) -> Result<Vec<u8>, ShellError> {
    match self {
      Self::Utf8 => Ok(data.into_bytes()),
      Self::Base64 => base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|error| ShellError::InvalidArguments(error.to_string())),
    }
  }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillMode {
  #[default]
  Graceful,
  Force,
}

pub(super) fn build_specifications() -> Vec<crate::protocol::Tool> {
  vec![
    crate::protocol::Tool {
      name: "shell_start".into(),
      description: "Start a shell command. If the command runs longer than timeout, it continues in the background and returns execution_id for later polling or termination. Output, exit code and diffs are captured. No interactive PTY is allocated.".into(),
      input_schema: serde_json::json!({
        "type": "object", "additionalProperties": false,
        "required": ["command"],
        "properties": {
          "command": {"type": "string", "description": "Shell script to execute."},
          "edit": {"type": "string", "description": "Optional absolute file path to compare before launch and after completion. Missing files are treated as empty, returning a unified diff in edit."},
          "timeout": {"type": "number", "description": "Seconds to wait for completion before backgrounding. Omit or use -1 for default. Use 0 to start and return immediately without waiting."},
          "data": {"type": "string", "description": "Initial data to feed into standard input."},
          "encoding": {"type": "string", "enum": ["utf8", "base64"], "default": "utf8"},
          "interactive": {"type": "boolean", "description": "Keep standard input open after launch to enable later shell_write calls."}
        }
      }),
    },
    crate::protocol::Tool {
      name: "shell_poll".into(),
      description: "Poll merged stdout/stderr and status of a running background command by its execution_id.".into(),
      input_schema: serde_json::json!({
        "type": "object", "additionalProperties": false,
        "required": ["execution_id"],
        "properties": {
          "execution_id": {"type": "string", "description": "Process identifier returned by shell_start."},
          "offset": {"type": "integer", "minimum": 0, "description": "Raw byte offset to read from."},
          "max_bytes": {"type": "integer", "minimum": 0, "description": "Maximum bytes to read; defaults to configured page size."},
          "wait_ms": {"type": "integer", "minimum": 0, "description": "Long-poll duration in milliseconds to wait for new output."},
          "encoding": {"type": "string", "enum": ["utf8", "base64"], "default": "utf8"}
        }
      }),
    },
    crate::protocol::Tool {
      name: "shell_write".into(),
      description: "Write data to standard input of an interactive background command.".into(),
      input_schema: serde_json::json!({
        "type": "object", "additionalProperties": false,
        "required": ["execution_id", "data"],
        "properties": {
          "execution_id": {"type": "string", "description": "Process identifier of an interactive execution."},
          "data": {"type": "string", "description": "Data to write to standard input."},
          "close": {"type": "boolean", "description": "Close standard input (EOF) after writing."},
          "encoding": {"type": "string", "enum": ["utf8", "base64"], "default": "utf8"}
        }
      }),
    },
    crate::protocol::Tool {
      name: "shell_kill".into(),
      description: "Terminate a background command.".into(),
      input_schema: serde_json::json!({
        "type": "object", "additionalProperties": false,
        "required": ["execution_id"],
        "properties": {
          "execution_id": {"type": "string", "description": "Process identifier to terminate."},
          "mode": {"type": "string", "enum": ["graceful", "force"], "default": "graceful"}
        }
      }),
    },
  ]
}
