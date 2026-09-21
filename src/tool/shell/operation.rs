use super::ShellError;
use base64::Engine;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShellOperation {
  Start {
    command: String,
    /// Soft wait seconds; -1 or omission selects ShellConfig.soft_timeout, 0 returns immediately.
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
  /// The model may omit operation when starting a command, as in old wish.
  pub fn parse(mut arguments: serde_json::Value) -> Result<Self, ShellError> {
    if let Some(object) = arguments.as_object_mut() {
      object.entry("operation").or_insert_with(|| "start".into());
    }
    serde_json::from_value(arguments)
      .map_err(|error| ShellError::InvalidArguments(error.to_string()))
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

pub(super) fn build_specification() -> crate::protocol::Tool {
  crate::protocol::Tool {
    name: "shell".into(),
    description: "Run shell scripts. Operations: start (default), poll, write, kill. Start timeout is a soft wait: a running command continues in the background and returns execution_id. Poll retrieves merged stdout/stderr by byte offset. Stdin defaults to EOF; use data for initial input or interactive=true to enable later writes. Nonzero exit codes are returned with output. No PTY is allocated.".into(),
    input_schema: serde_json::json!({
      "type":"object", "additionalProperties":false,
      "properties": {
        "operation":{"type":"string","enum":["start","poll","write","kill"],"default":"start"},
        "command":{"type":"string","description":"Shell script for start."},
        "timeout":{"type":"number","description":"Soft wait seconds for start; -1 uses default, 0 returns immediately."},
        "execution_id":{"type":"string","description":"Required by poll, write and kill."},
        "data":{"type":"string","description":"Initial stdin for start, or data for write."},
        "encoding":{"type":"string","enum":["utf8","base64"],"default":"utf8"},
        "interactive":{"type":"boolean","description":"Keep stdin open after start."},
        "close":{"type":"boolean","description":"Close stdin after write."},
        "offset":{"type":"integer","minimum":0,"description":"Raw byte offset for poll."},
        "max_bytes":{"type":"integer","minimum":0,"description":"Bytes to read for poll; defaults to configured page size."},
        "wait_ms":{"type":"integer","minimum":0,"description":"Long-poll duration."},
        "mode":{"type":"string","enum":["graceful","force"],"default":"graceful"}
      }
    }),
  }
}
