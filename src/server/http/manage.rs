//! Managing a session: editing it (`PATCH`), deleting and forking it, clearing its context, and
//! editing its queue.
use crate::application::session_edit;
use crate::application::{
  app::App,
  session::{CreateSession, ToolChanges},
};
use crate::server::error::{ApiError, blocking};
use crate::session::{EntryId, SessionError};
use axum::{
  Json,
  extract::{Path, State},
  http::{HeaderMap, StatusCode, header::IF_MATCH},
};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

/// Body: any of `name`, `provider` (with `config`), `config` and `metadata`, with `If-Match` naming
/// the revision the change was made against. A rename alone never waits for an operation; while
/// one runs, only the model selection may change, and it applies at the operation's next boundary.
pub async fn update_session(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
  Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  session_edit::update(
    app,
    slot,
    headers.get(IF_MATCH).map(|value| value.to_str().unwrap_or("").to_owned()),
    input,
  )
  .await
  .map(Json)
}
pub async fn delete_session(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
  session_edit::delete(&app, &id).await?;
  Ok(StatusCode::NO_CONTENT)
}
pub async fn cancel_input(
  State(app): State<Arc<App>>,
  Path((id, entry)): Path<(String, usize)>,
) -> Result<StatusCode, ApiError> {
  let slot = app.get_session(&id).await?;
  blocking(move || {
    slot.require_live()?;
    slot.sender.cancel_queued_input(EntryId(entry)).map_err(|error| match error {
      SessionError::InvalidEntry(_) => {
        ApiError::conflict("input has already been consumed or cancelled")
      }
      error => error.into(),
    })?;
    slot.persist_index()?;
    Ok(StatusCode::NO_CONTENT)
  })
  .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveInput {
  before: Option<usize>,
}
pub async fn move_input(
  State(app): State<Arc<App>>,
  Path((id, entry)): Path<(String, usize)>,
  Json(input): Json<MoveInput>,
) -> Result<StatusCode, ApiError> {
  let slot = app.get_session(&id).await?;
  blocking(move || {
    slot.require_live()?;
    slot.sender.move_queued_input(EntryId(entry), input.before.map(EntryId)).map_err(|error| {
      match error {
        SessionError::InvalidEntry(_) => {
          ApiError::conflict("queued input has already been consumed or cancelled")
        }
        error => error.into(),
      }
    })?;
    slot.persist_index()?;
    Ok(StatusCode::NO_CONTENT)
  })
  .await
}
/// Starts a new context generation that keeps only the fixed instructions at its front.
pub async fn clear_context(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  let slot = app.get_session(&id).await?;
  let mut session = slot.lock_idle_or_conflict().await?;
  blocking(move || {
    slot.require_live()?;
    let generation = session.get_active_generation()?;
    let entries = session.reader().get_generation_entry_ids(generation.id)?;
    let mut prefix = Vec::new();
    let mut position = 0;
    while let Some(id) = entries.get(position)? {
      let entry = session.reader().get_entry(*id)?.ok_or_else(ApiError::not_found)?;
      if !entry.message.is_fixed_instruction() {
        break;
      }
      prefix.push(*id);
      position += 1;
    }
    session.prepare_standby_generation(prefix)?;
    session.activate_standby_generation()?;
    Ok(Json(slot.publish(&session)?))
  })
  .await
}
/// Creates a copy of the session with its current context, settings and tool switches.
pub async fn fork(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  let slot = app.get_session(&id).await?;
  let session = slot.lock_idle_or_conflict().await?;
  let descriptor = slot.get_descriptor();
  let mut config = session.get_config().clone();
  config.tools.clear();
  let request = session.build_request()?;
  // Summaries and compaction items stay context in the copy, as they are here.
  let active = session.get_active_generation()?;
  let mut initial_origins = Vec::new();
  for id in session.reader().read_generation_entry_ids(active.id)? {
    let entry = session.reader().get_entry(id)?.ok_or(SessionError::InvalidEntry(id))?;
    initial_origins.push(entry.origin);
  }
  let input = CreateSession {
    created_by: None,
    folder: app.management.folder_of(&descriptor.id)?,
    name: format!("{} (copy)", descriptor.name),
    provider: descriptor.provider,
    cwd: descriptor.cwd,
    tools: ToolChanges::from(descriptor.tools),
    config,
    metadata: session.get_metadata().clone(),
    initial_messages: request.conversation,
    initial_origins,
  };
  drop(session);
  Ok((StatusCode::CREATED, Json(app.create_session(input).await?.describe())))
}
