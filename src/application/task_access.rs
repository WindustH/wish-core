//! Session-scoped task operations and completion notifications, independent of HTTP.
use super::{app::App, bridge_access, error::ApplicationError, session::SessionSlot};
use crate::{
  executor::ExecutionControl,
  session::ToolOutcome,
  task::{TaskError, TaskOperation},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum TaskRequest {
  List,
  BackgroundForeground,
  Show { task_id: String },
  Background { task_id: String },
  Remove { task_id: String },
  Execute { operation: TaskOperation },
}
/// The same safe defaults and actionable errors apply to tools and HTTP/CLI calls.
pub fn parse_request(arguments: &Value) -> Result<TaskRequest, ApplicationError> {
  let invalid = |reason: String| {
    ApplicationError::invalid(crate::tool::arguments::error("task", reason, r#"{"action":"list"}"#))
  };
  let mut schema = specification().input_schema;
  // An omitted action means the read-only list operation. Stdin whitespace is actual data.
  schema["required"] = serde_json::json!([]);
  schema["properties"]["operation"]["properties"]["data"]["x-preserve-whitespace"] = true.into();
  let mut value = crate::tool::arguments::normalize(arguments, &schema).map_err(invalid)?;
  if value.get("action").is_none_or(|action| {
    action.is_null() || action.as_str().is_some_and(|text| text.trim().is_empty())
  }) {
    value["action"] = "list".into();
  }
  if value["action"] == "execute" && value["operation"]["operation"] == "start" {
    if value["operation"].get("timeout").is_none() {
      value["operation"]["timeout"] = 0.into();
    }
  }
  serde_json::from_value(value)
    .map_err(|error| invalid(format!("{error}; run `wish task --help` for action-specific fields")))
}

fn failure(error: TaskError) -> ApplicationError {
  ApplicationError::invalid(error.to_string())
}
pub async fn as_session(
  app: Arc<App>,
  id: String,
  token: Option<&str>,
  request: TaskRequest,
) -> Result<Value, ApplicationError> {
  let slot = bridge_access::authenticate_session(&app, &id, token).await?;
  let call = crate::session::ToolCall {
    call_id: uuid::Uuid::new_v4().to_string(),
    name: "task".into(),
    arguments: serde_json::to_value(&request).map_err(ApplicationError::internal)?,
  };
  let trace = super::command_trace::CommandTrace::start(&app, slot.clone(), call).await?;
  let result = execute(&app, slot, request, false).await;
  let outcome = match &result {
    Ok(output) => ToolOutcome::Success(output.clone()),
    Err(error) => ToolOutcome::Failed(error.to_string()),
  };
  trace.finish(&outcome).await?;
  result
}
pub async fn as_user(
  app: Arc<App>,
  id: String,
  request: TaskRequest,
) -> Result<Value, ApplicationError> {
  let slot = app.get_session(&id).await?;
  execute(&app, slot, request, true).await
}
pub(crate) async fn execute(
  app: &Arc<App>,
  slot: Arc<SessionSlot>,
  request: TaskRequest,
  user_request: bool,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  slot.require_live()?;
  let launches =
    matches!(&request, TaskRequest::Execute { operation: TaskOperation::Start { .. } });
  if !slot.get_descriptor().tools.shell && (!user_request || launches) {
    return Err(ApplicationError::conflict("shell is disabled for this session"));
  }
  let shell = slot.tools.task_manager().await?;
  let epoch = slot.get_schedule_epoch();
  let result = match request {
    TaskRequest::BackgroundForeground => {
      let mut detached = Vec::new();
      for task in shell.list().map_err(failure)?["tasks"].as_array().into_iter().flatten() {
        if task["process"]["status"] == "running" && task["background"] == false {
          let id = task["task_id"].as_str().unwrap();
          // A task can finish between the snapshot and this operation.
          if let Ok(result) = shell.background(id).await {
            slot.tools.notify_background(&result, task["command"].clone(), Some(epoch)).await;
            detached.push(result);
          }
        }
      }
      serde_json::json!({"tasks":detached})
    }
    TaskRequest::List => shell.list().map_err(failure)?,
    TaskRequest::Show { task_id } => shell.show(&task_id).map_err(failure)?,
    TaskRequest::Remove { task_id } => shell.remove(&task_id).await.map_err(failure)?,
    TaskRequest::Background { task_id } => shell.background(&task_id).await.map_err(failure)?,
    TaskRequest::Execute { operation } => {
      let control = ExecutionControl::new();
      let guard = control.cancel_on_drop();
      let result = app.lifecycle.until_shutdown(shell.run(operation, &control)).await?;
      drop(guard);
      match result {
        ToolOutcome::Report { output, failed: false, .. } => output,
        ToolOutcome::Report { text, .. } => return Err(ApplicationError::invalid(text)),
        ToolOutcome::Success(output) => output,
        ToolOutcome::SuccessWithMetadata { output, .. } => output,
        ToolOutcome::Failed(message) => return Err(ApplicationError::invalid(message)),
        ToolOutcome::Unknown(message) => return Err(ApplicationError::internal(message)),
        ToolOutcome::Cancelled => {
          return Err(ApplicationError::conflict("task operation cancelled"));
        }
        _ => return Err(ApplicationError::internal("unexpected task result")),
      }
    }
  };
  if result["process"]["status"] == "running" {
    if let Some(id) = result["execution_id"].as_str() {
      let task = shell.show(id).map_err(failure)?;
      if task["background"] == true {
        slot.tools.notify_background(&result, task["command"].clone(), Some(epoch)).await;
      }
    }
  }
  Ok(result)
}

/// Task management is one optional tool; its argument protocol is also used by the CLI bridge.
pub fn specification() -> crate::protocol::Tool {
  crate::protocol::Tool {
    name: "task".into(),
    description: "Manage this session's shell tasks. List/show tasks, background a running shell without restarting it, remove completed tasks, or execute start/poll/write/kill. For execute, provide an operation object with an operation name and its arguments. start accepts command, timeout (soft wait in seconds, 0 returns immediately), interactive, data, encoding; poll accepts execution_id, offset, max_bytes, wait_ms, encoding; write accepts execution_id, data, close, encoding; kill accepts execution_id, mode (graceful/force).".into(),
    input_schema: serde_json::json!({"type":"object","required":["action"],"properties":{
      "action":{"type":"string","enum":["list","show","background","remove","execute"]},
      "task_id":{"type":"string"},"operation":{"type":"object","required":["operation"],"properties":{
        "operation":{"type":"string","enum":["start","poll","write","kill"]},
        "command":{"type":"string"},"timeout":{"type":"number"},"interactive":{"type":"boolean"},
        "execution_id":{"type":"string"},"data":{"type":"string"},"close":{"type":"boolean"},
        "encoding":{"type":"string","enum":["utf8","base64"]},"offset":{"type":"integer","minimum":0},
        "max_bytes":{"type":"integer","minimum":0},"wait_ms":{"type":"integer","minimum":0},
        "mode":{"type":"string","enum":["graceful","force"]}},"additionalProperties":false}},"additionalProperties":false}),
  }
}
