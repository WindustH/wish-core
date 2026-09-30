//! Managing a session: editing it (`PATCH`), deleting and forking it, clearing its context, and
//! editing its queue.
use crate::server::{
  app::App,
  compaction_item,
  error::{ApiError, blocking},
  session::{CreateSession, SessionSlot, ToolChanges, selection::PendingSelection},
};
use crate::session::{EntryId, Session, SessionConfig, SessionError};
use axum::{
  Json,
  extract::{Path, State},
  http::{HeaderMap, HeaderValue, StatusCode, header::IF_MATCH},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::OwnedMutexGuard;

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
  let _update = slot.update_lock.clone().lock_owned().await;
  let if_match = headers.get(IF_MATCH).cloned();
  // Names belong to the descriptor, not the executing session. Do not wait
  // for the execution mutex or disturb a pending model selection.
  if input.as_object().is_some_and(|object| object.len() == 1 && object.contains_key("name")) {
    return rename(slot, if_match, &input).await;
  }
  match slot.lock_idle().await {
    None => stage_selection(app, slot, if_match, input).await,
    Some(session) => update_idle(app, slot, session, if_match, input).await,
  }
}
async fn rename(
  slot: Arc<SessionSlot>,
  if_match: Option<HeaderValue>,
  input: &Value,
) -> Result<Json<Value>, ApiError> {
  let name = input["name"]
    .as_str()
    .ok_or_else(|| ApiError::bad_request("name must be a string"))?
    .to_owned();
  blocking(move || {
    slot.require_live()?;
    slot.edit_descriptor(if_match.as_ref(), |next| {
      next.name = name;
      Ok(())
    })?;
    Ok(Json(slot.describe()))
  })
  .await
}
async fn stage_selection(
  app: Arc<App>,
  slot: Arc<SessionSlot>,
  if_match: Option<HeaderValue>,
  input: Value,
) -> Result<Json<Value>, ApiError> {
  let object = input.as_object().ok_or_else(|| ApiError::bad_request("expected an object"))?;
  if object.keys().any(|key| !["provider", "config"].contains(&key.as_str())) {
    return Err(ApiError::conflict("only model and reasoning settings can change while running"));
  }
  let config: SessionConfig = serde_json::from_value(
    input.get("config").cloned().ok_or_else(|| ApiError::bad_request("config required"))?,
  )
  .map_err(|e| ApiError::bad_request(e.to_string()))?;
  let config = slot.configure_tools(config)?;
  blocking(move || {
    slot.require_live()?;
    slot.stage_selection(&app, if_match.as_ref(), input.get("provider"), config)?;
    Ok(Json(slot.describe()))
  })
  .await
}
/// What a config change does to the descriptor's provider.
enum Selection {
  /// Staged, for an encrypted item to be translated at the next run boundary.
  Staged(PendingSelection),
  /// Made now.
  Made(String),
}
async fn update_idle(
  app: Arc<App>,
  slot: Arc<SessionSlot>,
  mut session: OwnedMutexGuard<Session>,
  if_match: Option<HeaderValue>,
  input: Value,
) -> Result<Json<Value>, ApiError> {
  slot.require_live()?;
  let object = input.as_object().ok_or_else(|| ApiError::bad_request("expected an object"))?;
  if let Some(key) =
    object.keys().find(|key| !["name", "provider", "config", "metadata"].contains(&key.as_str()))
  {
    return Err(ApiError::bad_request(format!("unknown field: {key}")));
  }
  slot.check_revision(if_match.as_ref())?;
  let name = input
    .get("name")
    .map(|name| name.as_str().ok_or_else(|| ApiError::bad_request("name must be a string")))
    .transpose()?
    .map(str::to_owned);
  let descriptor = slot.get_descriptor();
  let mut provider = descriptor.pending_selection.map_or(descriptor.provider, |p| p.provider);
  if let Some(value) = input.get("provider") {
    provider =
      value.as_str().ok_or_else(|| ApiError::bad_request("provider must be a string"))?.to_owned();
    app.get_provider(&provider)?;
    if input.get("config").is_none() {
      return Err(ApiError::bad_request("provider changes require config"));
    }
  }
  let config = input
    .get("config")
    .map(|v| serde_json::from_value(v.clone()).map_err(|e| ApiError::bad_request(e.to_string())))
    .transpose()?;
  let config = config.map(|config| slot.configure_tools(config)).transpose()?;
  let metadata = input.get("metadata").cloned();
  blocking(move || {
    let mut selection = None;
    if let Some(config) = config {
      app.get_provider(&provider)?;
      // An encrypted item the next provider cannot read, and that has no handoff yet, is
      // translated at the next run boundary, while the current provider can still read it.
      let conversation = session.build_request()?.conversation;
      if conversation.iter().any(|message| compaction_item::needs_handoff(message, &provider)) {
        selection = Some(Selection::Staged(PendingSelection { provider, config }));
      } else {
        session.set_config(config)?;
        selection = Some(Selection::Made(provider));
      }
    }
    if let Some(metadata) = metadata {
      session.set_metadata(metadata)?;
    }
    slot.refresh_status(&session);
    slot.edit_descriptor(None, |next| {
      if let Some(name) = name {
        next.name = name;
      }
      match selection {
        Some(Selection::Staged(pending)) => next.pending_selection = Some(pending),
        Some(Selection::Made(provider)) => {
          next.provider = provider;
          next.pending_selection = None;
        }
        None => {}
      }
      Ok(())
    })?;
    Ok(Json(slot.describe()))
  })
  .await
}
pub async fn delete_session(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let mut session = slot
    .lock_idle()
    .await
    .ok_or_else(|| ApiError::conflict("interrupt and wait for the session before deleting"))?;
  slot.require_live()?;
  if let Some(shell) = slot.tools.shell() {
    shell.shutdown().await.map_err(ApiError::internal)?;
  }
  app.mcp.close_session(&id);
  let (management, target, deleted) = (app.management.clone(), id.clone(), slot.clone());
  blocking(move || {
    session.delete()?;
    deleted.mark_deleted();
    management.delete(&target)?;
    Ok(())
  })
  .await?;
  app.sessions.lock().await.remove(&id);
  for directory in [app.data_dir.shell(&id), app.data_dir.blobs(&id)] {
    match tokio::fs::remove_dir_all(directory).await {
      Ok(()) => {}
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
      Err(e) => return Err(ApiError::internal(e)),
    }
  }
  let _ = slot.events.send(json!({"type":"deleted"}));
  let _ = app.events.send(json!({"type":"session_deleted","id":id}));
  // The session is gone either way; this only hands its room back to the file system.
  let storage = app.storage.clone();
  if let Err(error) = blocking(move || storage.shrink().map_err(ApiError::internal)).await {
    eprintln!("shrink storage after deleting {id}: {}", error.message);
  }
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
    name: format!("{} (copy)", descriptor.name),
    provider: descriptor.provider,
    cwd: descriptor.cwd,
    tools: ToolChanges {
      shell: Some(descriptor.tools.shell),
      ask_user: Some(descriptor.tools.ask_user),
      mcp: Some(descriptor.tools.mcp),
      web_search: Some(descriptor.tools.web_search),
    },
    config,
    metadata: session.get_metadata().clone(),
    initial_messages: request.conversation,
    initial_origins,
  };
  drop(session);
  Ok((StatusCode::CREATED, Json(app.create_session(input).await?.describe())))
}
