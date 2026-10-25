//! MCP access: the bridge a session's shell reaches its servers through, and the settings page's
//! view of them.
//!
//! A session's MCP switch is enforced here and nowhere else; the token the bridge answers is
//! checked in `bridge`.
use super::bridge_access as bridge;
use crate::application::error::ApplicationError;
use crate::application::{
  app::App,
  mcp::{Caller, store_binary_content},
  provider::Providers,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// The session a bridge request speaks for, when its MCP switch is on, and the providers as they
/// stand.
async fn bridge_session(
  app: &Arc<App>,
  id: &str,
  token: Option<&str>,
) -> Result<(Caller, Providers), ApplicationError> {
  let slot = bridge::authenticate_session(app, id, token).await?;
  // Said to the model through the command's output, so it knows why and who can change it.
  let descriptor = slot.get_descriptor();
  if !descriptor.tools.mcp {
    return Err(ApplicationError::conflict(
      "MCP is disabled for this session. The user can enable it in the session's settings.",
    ));
  }
  Ok((Caller { session: descriptor.id, cwd: descriptor.cwd }, app.get_providers()))
}

#[derive(Deserialize)]
pub struct ServersQuery {
  server: Option<String>,
}
pub async fn servers(
  app: Arc<App>,
  id: String,
  query: ServersQuery,
  token: Option<&str>,
) -> Result<Value, ApplicationError> {
  let (caller, providers) = bridge_session(&app, &id, token).await?;
  Ok(json!(app.mcp.list(&caller, &providers, query.server.as_deref()).await?))
}

#[derive(Deserialize)]
pub struct ToolQuery {
  server: String,
  name: String,
}
pub async fn tool(
  app: Arc<App>,
  id: String,
  query: ToolQuery,
  token: Option<&str>,
) -> Result<Value, ApplicationError> {
  let (caller, providers) = bridge_session(&app, &id, token).await?;
  Ok(app.mcp.describe_tool(&query.server, &query.name, &caller, &providers).await?)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallInput {
  server: String,
  tool: String,
  #[serde(default)]
  arguments: Map<String, Value>,
}
/// Body: `{"server", "tool", "arguments"}`. Returns the tool's result, with binary content saved as
/// files. The call stops when the request does: a command interrupted in the shell drops it, and the
/// server is told.
pub async fn call(
  app: Arc<App>,
  id: String,
  token: Option<&str>,
  input: CallInput,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let (caller, providers) = bridge_session(&app, &id, token).await?;
  let call = app.mcp.call(&input.server, &input.tool, input.arguments, &caller, &providers);
  let mut result = app.lifecycle.until_shutdown(call).await??;
  let directory =
    std::path::absolute(app.data_dir.mcp_files(&id)).map_err(ApplicationError::internal)?;
  store_binary_content(&mut result, &directory);
  Ok(result)
}

/// Every configured server with what is known of it: tools, the last error, recent stderr.
pub async fn list(app: Arc<App>) -> Value {
  json!(app.mcp.describe())
}

/// Connects to a server on a connection of its own and lists its tools.
pub async fn check(app: Arc<App>, id: String) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let providers = app.get_providers();
  Ok(app.lifecycle.until_shutdown(app.mcp.check(&id, &providers)).await??)
}
