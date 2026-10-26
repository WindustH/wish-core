//! The model-facing command tool. Process ownership and task operations live in `task`.
pub use crate::task::{
  ShellCommand, TaskConfig as ShellConfig, TaskError as ShellError, is_runnable,
};
use crate::{
  executor::{ExecutionControl, tool::ToolExecutor},
  protocol::Tool,
  session::{ToolCall, ToolOutcome},
  task::TaskManager,
};
use serde::Deserialize;
#[derive(Clone)]
pub struct ShellTool {
  tasks: TaskManager,
}
impl std::ops::Deref for ShellTool {
  type Target = TaskManager;
  fn deref(&self) -> &TaskManager {
    &self.tasks
  }
}
impl ShellTool {
  pub async fn new(config: ShellConfig) -> Result<Self, ShellError> {
    Ok(Self { tasks: TaskManager::new(config).await? })
  }
  pub fn get_specifications(&self) -> Vec<Tool> {
    vec![Tool {
      name: "shell".into(),
      description: format!(
        "Execute a shell command in the session working directory and wait for completion. Optional timeout is a hard limit in seconds; omitted means no time limit. Each call uses a new shell, without a PTY. Output is limited to its last 2000 lines / 50 KiB; the full output remains available. For background jobs, stdin, task management use `wish task --help`. All commands are registered as tasks. `wish task background <id>` transfers a running command to the background without restarting it. {}",
        crate::task::SHELL_DESCRIPTION_NOTE
      ),
      input_schema: serde_json::json!({"type":"object","additionalProperties":false,"required":["command"],"properties":{"command":{"type":"string"},"edit":{"type":"array","items":{"type":"string"},"description":"Absolute paths to compare before and after the command."},"check_diff":{"type":"boolean","default":false,"description":"Include bounded file diffs in the result; omitted portions remain in the task record. Otherwise return change summaries."},"timeout":{"type":"number","exclusiveMinimum":0,"description":"Hard timeout in seconds. Omit for no timeout."}}}),
    }]
  }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
  command: String,
  timeout: Option<f64>,
  #[serde(default)]
  edit: Vec<std::path::PathBuf>,
  #[serde(default)]
  check_diff: bool,
}
impl ToolExecutor for ShellTool {
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    self.execute_responsive(call, control, None).await
  }
  async fn execute_with_input(&self, call: &ToolCall, control: &ExecutionControl,
    input: Option<crate::executor::tool::ToolInput>) -> ToolOutcome {
    self.execute_responsive(call, control, input).await
  }
}
impl ShellTool {
  async fn execute_responsive(&self, call: &ToolCall, control: &ExecutionControl,
    input: Option<crate::executor::tool::ToolInput>) -> ToolOutcome {
    let arrival = input;
    if call.name == "shell_edit" {
      if !call.arguments["diff"].as_array().is_some_and(|paths| !paths.is_empty())
        || call.arguments.get("timeout").is_some()
      {
        return ToolOutcome::Failed(
          "legacy shell_edit requires diff paths and does not accept timeout".into(),
        );
      }
      let mut arguments = call.arguments.clone();
      if let Some(object) = arguments.as_object_mut() {
        if let Some(paths) = object.remove("diff") {
          object.insert("edit".into(), paths);
        }
      }
      let input: Command = match serde_json::from_value(arguments) {
        Ok(input) => input,
        Err(error) => return ToolOutcome::Failed(error.to_string()),
      };
      return self
        .tasks
        .foreground_responsive(input.command, input.timeout, input.edit, input.check_diff, control, arrival)
        .await
        .unwrap_or_else(Into::into);
    }
    if call.name != "shell" {
      return match parse_legacy_call(&call.name, call.arguments.clone()) {
        Ok(operation) => self.tasks.run(operation, control).await,
        Err(error) => ToolOutcome::Failed(error.to_string()),
      };
    }
    let parsed =
      super::arguments::normalize(&call.arguments, &self.get_specifications()[0].input_schema)
        .and_then(|value| {
          serde_json::from_value::<Command>(value).map_err(|error| error.to_string())
        });
    let input = match parsed {
      Ok(input) => input,
      Err(error) => {
        return ToolOutcome::Failed(super::arguments::error(
          "shell",
          error,
          r#"{"command":"pwd"}"#,
        ));
      }
    };
    if input.command.trim().is_empty() {
      return ToolOutcome::Failed(super::arguments::error(
        "shell",
        "`command` must be a non-empty string",
        r#"{"command":"pwd"}"#,
      ));
    }
    self
      .tasks
      .foreground_responsive(input.command, input.timeout, input.edit, input.check_diff, control, arrival)
      .await
      .unwrap_or_else(Into::into)
  }
}

fn parse_legacy_call(
  name: &str,
  mut arguments: serde_json::Value,
) -> Result<crate::task::TaskOperation, ShellError> {
  let operation = match name {
    "shell_start" => "start",
    "shell_poll" => "poll",
    "shell_write" => "write",
    "shell_kill" => "kill",
    _ => return Err(ShellError::InvalidArguments(format!("unknown shell tool: {name}"))),
  };
  if let Some(object) = arguments.as_object_mut() {
    object.insert("operation".into(), operation.into());
  }
  serde_json::from_value(arguments).map_err(|error| ShellError::InvalidArguments(error.to_string()))
}

#[cfg(test)]
mod argument_tests {
  use super::*;
  use serde_json::json;
  #[tokio::test]
  async fn optional_empty_values_execute_but_missing_command_does_not() {
    let directory = std::env::temp_dir().join(format!("wish-shell-args-{}", uuid::Uuid::new_v4()));
    let tool = ShellTool::new(ShellConfig::new(std::env::temp_dir(), &directory)).await.unwrap();
    let call = |arguments| ToolCall { call_id: "shell".into(), name: "shell".into(), arguments };
    let outcome = tool
      .execute(
        &call(json!({"command":"echo fallback","edit":null,"timeout":"","check_diff":" "})),
        &ExecutionControl::new(),
      )
      .await;
    assert!(
      matches!(outcome, ToolOutcome::Report { ref output, failed: false, .. } if output["text"].as_str().unwrap().contains("fallback")),
      "{outcome:?}"
    );
    for arguments in [json!(null), json!({"command":" "}), json!({"command":42})] {
      let outcome = tool.execute(&call(arguments), &ExecutionControl::new()).await;
      assert!(
        matches!(outcome, ToolOutcome::Failed(message) if message.contains("command") && message.contains("Example:"))
      );
    }
    assert_eq!(tool.list().unwrap()["tasks"].as_array().unwrap().len(), 1);
    tokio::fs::remove_dir_all(directory).await.unwrap();
  }
}
