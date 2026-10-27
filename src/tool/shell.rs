//! The model-facing command tool. Process ownership and task operations live in `task`.
use crate::{
  protocol::Tool,
  session::{ToolCall, ToolOutcome},
  task::{TaskConfig, TaskError, TaskManager},
  tool::contract::ToolExecutor,
  utils::control::ExecutionControl,
};
use serde::Deserialize;
#[derive(Clone)]
pub struct ShellTool {
  tasks: TaskManager,
}
impl ShellTool {
  pub fn from_tasks(tasks: TaskManager) -> Self {
    Self { tasks }
  }
  #[allow(dead_code)] // external integration fixtures
  pub fn tasks(&self) -> &TaskManager {
    &self.tasks
  }
  #[allow(dead_code)] // external integration fixtures
  pub async fn new(config: TaskConfig) -> Result<Self, TaskError> {
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
  async fn execute_with_input(
    &self,
    call: &ToolCall,
    control: &ExecutionControl,
    input: Option<crate::session::ToolInput>,
  ) -> ToolOutcome {
    self.execute_responsive(call, control, input).await
  }
}
impl ShellTool {
  async fn execute_responsive(
    &self,
    call: &ToolCall,
    control: &ExecutionControl,
    input: Option<crate::session::ToolInput>,
  ) -> ToolOutcome {
    let arrival: Option<crate::task::InputArrival> = input.map(|mut input| {
      Box::pin(async move { input.wait().await.map_err(|error| error.to_string()) })
        as crate::task::InputArrival
    });
    if call.name == "shell_edit" {
      if !call.arguments["diff"].as_array().is_some_and(|paths| !paths.is_empty())
        || call.arguments.get("timeout").is_some()
      {
        return ToolOutcome::Failed(
          "legacy shell_edit requires diff paths and does not accept timeout".into(),
        );
      }
      let mut arguments = call.arguments.clone();
      if let Some(object) = arguments.as_object_mut()
        && let Some(paths) = object.remove("diff")
      {
        object.insert("edit".into(), paths);
      }
      let input: Command = match serde_json::from_value(arguments) {
        Ok(input) => input,
        Err(error) => return ToolOutcome::Failed(error.to_string()),
      };
      return self
        .tasks
        .foreground_responsive(
          input.command,
          input.timeout,
          input.edit,
          input.check_diff,
          control,
          arrival,
        )
        .await
        .map(|result| super::task_output::foreground(result, input.check_diff))
        .unwrap_or_else(Into::into);
    }
    if call.name != "shell" {
      return match parse_legacy_call(&call.name, call.arguments.clone()) {
        Ok(operation) => super::task_output::operation(self.tasks.run(operation, control).await),
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
      .foreground_responsive(
        input.command,
        input.timeout,
        input.edit,
        input.check_diff,
        control,
        arrival,
      )
      .await
      .map(|result| super::task_output::foreground(result, input.check_diff))
      .unwrap_or_else(Into::into)
  }
}

fn parse_legacy_call(
  name: &str,
  mut arguments: serde_json::Value,
) -> Result<crate::task::TaskOperation, TaskError> {
  let operation = match name {
    "shell_start" => "start",
    "shell_poll" => "poll",
    "shell_write" => "write",
    "shell_kill" => "kill",
    _ => return Err(TaskError::InvalidArguments(format!("unknown shell tool: {name}"))),
  };
  if let Some(object) = arguments.as_object_mut() {
    object.insert("operation".into(), operation.into());
  }
  serde_json::from_value(arguments).map_err(|error| TaskError::InvalidArguments(error.to_string()))
}
