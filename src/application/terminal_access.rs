//! User terminal access uses the same session shell configuration and lifecycle as model tasks.
use super::{app::App, error::ApplicationError};
use crate::task::TerminalRequest;
use serde_json::Value;
use std::sync::Arc;

pub async fn execute(
  app: Arc<App>,
  id: String,
  request: TerminalRequest,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  slot.require_live()?;
  if matches!(request, TerminalRequest::Start { .. } | TerminalRequest::Write { .. })
    && !slot.get_descriptor().tools.shell
  {
    return Err(ApplicationError::conflict("shell is disabled for this session"));
  }
  if matches!(request, TerminalRequest::Start { .. }) {
    slot.ensure_working_directory().await?;
  }
  let manager = slot.tools.task_manager().await?;
  let settings = app.config_file.lock().await.config.terminal.clone();
  let mut result =
    app.lifecycle.until_shutdown(manager.terminal(request, settings.clone())).await??;
  result["settings"] = serde_json::to_value(settings).map_err(ApplicationError::internal)?;
  Ok(result)
}

/// Authenticate and pin the session before upgrading a dedicated PTY connection.
pub async fn connect(
  app: &Arc<App>,
  id: &str,
  terminal_id: &str,
) -> Result<Arc<crate::task::Terminal>, ApplicationError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(id).await?;
  slot.require_live()?;
  if !slot.get_descriptor().tools.shell {
    return Err(ApplicationError::conflict("shell is disabled for this session"));
  }
  Ok(slot.tools.task_manager().await?.connected_terminal(terminal_id)?)
}
