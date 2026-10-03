//! Who a bridge request speaks for. The bridge answers a session's own token, which only that
//! session's shell holds, and never the application's: a command a session runs can reach that
//! session's MCP servers and skills and nothing else. Each feature's switch is checked by its own
//! handlers.
use crate::server::{app::App, error::ApiError, session::SessionSlot};
use axum::http::{HeaderMap, header::AUTHORIZATION};
use std::sync::Arc;

/// The session a bridge request speaks for, when its token is the one that session's shell holds.
pub async fn session(
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
  let Some(slot) = slot.filter(|slot| Some(slot.bridge_token.as_str()) == supplied) else {
    return Err(ApiError::unauthorized());
  };
  slot.require_live()?;
  Ok(slot)
}
