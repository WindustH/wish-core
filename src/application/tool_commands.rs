//! Session-authenticated command access to the same capabilities exposed as model tools.
use super::{app::App, bridge_access, error::ApplicationError};
use crate::{
  executor::{ExecutionControl, tool::ToolExecutor},
  protocol::Tool,
  session::{ToolCall, ToolOutcome},
};
use serde_json::Value;
use std::sync::Arc;

pub async fn execute(
  app: Arc<App>,
  id: String,
  token: Option<&str>,
  name: String,
  arguments: Option<Value>,
) -> Result<Value, ApplicationError> {
  let slot = bridge_access::authenticate_session(&app, &id, token).await?;
  app.lifecycle.require_open()?;
  let switches = slot.get_descriptor().tools;
  if !switches.shell {
    return Err(ApplicationError::conflict("shell is disabled for this session"));
  }
  let (enabled, specification): (bool, Tool) = match name.as_str() {
    "history_search" | "history_read" | "history_query" => (switches.history_search,
      slot.tools.history.get_specifications().into_iter().find(|tool| tool.name == name)
        .ok_or_else(ApplicationError::not_found)?),
    "webfetch" => (switches.web_fetch, slot.tools.web_fetch.get_specification()),
    "websearch" => (switches.web_search, slot.tools.web_search.get_specification()),
    "askuser" => (switches.ask_user, slot.tools.ask_user.get_specification()),
    _ => return Err(ApplicationError::not_found()),
  };
  if !enabled {
    return Err(ApplicationError::conflict(format!("{name} is disabled for this session")));
  }
  let Some(arguments) = arguments else {
    return Ok(
      serde_json::json!({"description":specification.description,"input_schema":specification.input_schema}),
    );
  };
  let call =
    ToolCall { call_id: uuid::Uuid::new_v4().to_string(), name: specification.name, arguments };
  let trace = super::command_trace::CommandTrace::start(&app, slot.clone(), call.clone()).await?;
  let control = ExecutionControl::new();
  let _cancel = control.cancel_on_drop();
  let outcome = app.lifecycle.until_shutdown(slot.tools.execute(&call, &control)).await?;
  trace.finish(&outcome).await?;
  match outcome {
    ToolOutcome::Report { output, failed: false, .. } => Ok(output),
    ToolOutcome::Report { text, .. } => Err(ApplicationError::invalid(text)),
    ToolOutcome::Success(output) | ToolOutcome::SuccessWithMetadata { output, .. } => Ok(output),
    ToolOutcome::Failed(message) => Err(ApplicationError::invalid(message)),
    ToolOutcome::Cancelled => Err(ApplicationError::conflict("command cancelled")),
    ToolOutcome::Unknown(message) => Err(ApplicationError::internal(message)),
    _ => Err(ApplicationError::internal("unsupported command result")),
  }
}
