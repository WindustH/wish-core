//! MCP over HTTP: the bridge a session's shell reaches its servers through, and the settings page's
//! view of them.
//!
//! The bridge answers a session's own token, which only that session's shell holds, and never the
//! application's: a command a session runs can reach that session's servers and nothing else. A
//! session's MCP switch is enforced here and nowhere else.
use crate::server::{
  app::App,
  error::ApiError,
  mcp::{Caller, store_binary_content},
  session::SessionSlot,
};
use axum::{
  Json,
  extract::{Path, Query, State},
  http::{HeaderMap, StatusCode, header::AUTHORIZATION},
};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::sync::Arc;

/// The session a bridge request speaks for, when its token is the one that session's shell holds.
async fn bridge_session(
  app: &Arc<App>,
  id: &str,
  headers: &HeaderMap,
) -> Result<Arc<SessionSlot>, ApiError> {
  let supplied = headers
    .get(AUTHORIZATION)
    .and_then(|value| value.to_str().ok())
    .and_then(|value| value.strip_prefix("Bearer "));
  // A shell holding a token belongs to a session that is open, so there is nothing to load.
  let slot = app.sessions.lock().await.get(id).cloned();
  let Some(slot) = slot.filter(|slot| Some(slot.mcp_token.as_str()) == supplied) else {
    return Err(ApiError {
      status: StatusCode::UNAUTHORIZED,
      message: "unauthorized".into(),
      details: None,
    });
  };
  slot.require_live()?;
  // Said to the model through the command's output, so it knows why and who can change it.
  if !slot.get_descriptor().tools.mcp {
    return Err(ApiError::conflict(
      "MCP is disabled for this session. The user can enable it in the session's settings.",
    ));
  }
  Ok(slot)
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
  let slot = bridge_session(&app, &id, &headers).await?;
  let cwd = slot.get_descriptor().cwd;
  let providers = app.providers.read().unwrap().clone();
  let caller = Caller { session: &id, cwd: &cwd };
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
  let slot = bridge_session(&app, &id, &headers).await?;
  let cwd = slot.get_descriptor().cwd;
  let providers = app.providers.read().unwrap().clone();
  let caller = Caller { session: &id, cwd: &cwd };
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
  app.require_open()?;
  let slot = bridge_session(&app, &id, &headers).await?;
  let cwd = slot.get_descriptor().cwd;
  let providers = app.providers.read().unwrap().clone();
  let caller = Caller { session: &id, cwd: &cwd };
  let mut result = tokio::select! {
    _ = app.stop.cancelled() => return Err(ApiError::conflict("server is shutting down")),
    result = app.mcp.call(&input.server, &input.tool, input.arguments, &caller, &providers) => result?,
  };
  let directory = std::path::absolute(app.data_dir.join("shell").join(&id).join("mcp"))
    .map_err(ApiError::internal)?;
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
  app.require_open()?;
  let providers = app.providers.read().unwrap().clone();
  tokio::select! {
    _ = app.stop.cancelled() => Err(ApiError::conflict("server is shutting down")),
    result = app.mcp.check(&id, &providers) => Ok(Json(result?)),
  }
}
