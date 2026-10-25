//! Authorization for commands issued by a session's shell.
use super::{app::App, error::ApplicationError, session::SessionSlot};
use std::sync::Arc;
pub async fn authenticate_session(
  app: &Arc<App>,
  id: &str,
  supplied: Option<&str>,
) -> Result<Arc<SessionSlot>, ApplicationError> {
  // A shell holding a token belongs to a session that is open, so there is nothing to load.
  let slot = app.sessions.lock().await.get(id).cloned();
  let Some(slot) = slot.filter(|slot| Some(slot.bridge_token.as_str()) == supplied) else {
    return Err(ApplicationError::unauthorized());
  };
  slot.require_live()?;
  Ok(slot)
}
