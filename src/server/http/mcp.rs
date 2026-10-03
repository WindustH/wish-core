//! MCP over HTTP: the bridge a session's shell reaches its servers through, and the settings page's
//! view of them.
//!
//! A session's MCP switch is enforced here and nowhere else; the token the bridge answers is
//! checked in `bridge`.
use super::bridge;
use crate::server::{
  app::App,
  error::ApiError,
  mcp::{Caller, store_binary_content},
  provider::Providers,
};
use axum::{
  Json,
  extract::{Path, Query, State},
  http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// The session a bridge request speaks for, when its MCP switch is on, and the providers as they
/// stand.
async fn bridge_session(
  app: &Arc<App>,
  id: &str,
  headers: &HeaderMap,
) -> Result<(Caller, Providers), ApiError> {
  let slot = bridge::session(app, id, headers).await?;
  // Said to the model through the command's output, so it knows why and who can change it.
  let descriptor = slot.get_descriptor();
  if !descriptor.tools.mcp {
    return Err(ApiError::conflict(
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
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<ServersQuery>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  let (caller, providers) = bridge_session(&app, &id, &headers).await?;
  Ok(Json(json!(app.mcp.list(&caller, &providers, query.server.as_deref()).await?)))
}

#[derive(Deserialize)]
pub struct ToolQuery {
  server: String,
  name: String,
}
pub async fn tool(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<ToolQuery>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  let (caller, providers) = bridge_session(&app, &id, &headers).await?;
  Ok(Json(app.mcp.describe_tool(&query.server, &query.name, &caller, &providers).await?))
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
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
  Json(input): Json<CallInput>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  let (caller, providers) = bridge_session(&app, &id, &headers).await?;
  let call = app.mcp.call(&input.server, &input.tool, input.arguments, &caller, &providers);
  let mut result = app.lifecycle.until_shutdown(call).await??;
  let directory = std::path::absolute(app.data_dir.mcp_files(&id)).map_err(ApiError::internal)?;
  store_binary_content(&mut result, &directory);
  Ok(Json(result))
}

/// Every configured server with what is known of it: tools, the last error, recent stderr.
pub async fn list(State(app): State<Arc<App>>) -> Json<Value> {
  Json(json!(app.mcp.describe()))
}

/// Connects to a server on a connection of its own and lists its tools.
pub async fn check(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  let providers = app.get_providers();
  Ok(Json(app.lifecycle.until_shutdown(app.mcp.check(&id, &providers)).await??))
}
