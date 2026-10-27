//! Session edits shared by HTTP and peer commands. Owns revision checks and execution coordination.
use super::{
  app::App,
  compaction_item,
  error::{ApplicationError, blocking},
  session::{SessionSlot, selection::PendingSelection},
};
use crate::session::{Session, SessionConfig};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::OwnedMutexGuard;
/// Changes a session as `PATCH /sessions/{id}` does, for the user or through the bridge.
pub async fn update(
  app: Arc<App>,
  slot: Arc<SessionSlot>,
  if_match: Option<String>,
  input: Value,
) -> Result<Value, ApplicationError> {
  let _update = slot.update_lock.clone().lock_owned().await;
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
  if_match: Option<String>,
  input: &Value,
) -> Result<Value, ApplicationError> {
  let name = input["name"]
    .as_str()
    .ok_or_else(|| ApplicationError::invalid("name must be a string"))?
    .to_owned();
  blocking(move || {
    slot.require_live()?;
    slot.edit_descriptor(if_match.as_deref(), |next| {
      next.name = name;
      Ok(())
    })?;
    Ok(slot.describe())
  })
  .await
}
async fn stage_selection(
  app: Arc<App>,
  slot: Arc<SessionSlot>,
  if_match: Option<String>,
  input: Value,
) -> Result<Value, ApplicationError> {
  let object = input.as_object().ok_or_else(|| ApplicationError::invalid("expected an object"))?;
  if object.keys().any(|key| !["provider", "config"].contains(&key.as_str())) {
    return Err(ApplicationError::conflict(
      "only model and reasoning settings can change while running",
    ));
  }
  let config: SessionConfig = serde_json::from_value(
    input.get("config").cloned().ok_or_else(|| ApplicationError::invalid("config required"))?,
  )
  .map_err(|e| ApplicationError::invalid(e.to_string()))?;
  let config = slot.configure_tools(config)?;
  blocking(move || {
    slot.require_live()?;
    slot.stage_selection(&app, if_match.as_deref(), input.get("provider"), config)?;
    Ok(slot.describe())
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
  if_match: Option<String>,
  input: Value,
) -> Result<Value, ApplicationError> {
  slot.require_live()?;
  let object = input.as_object().ok_or_else(|| ApplicationError::invalid("expected an object"))?;
  if let Some(key) =
    object.keys().find(|key| !["name", "provider", "config", "metadata"].contains(&key.as_str()))
  {
    return Err(ApplicationError::invalid(format!("unknown field: {key}")));
  }
  slot.check_revision(if_match.as_deref())?;
  let name = input
    .get("name")
    .map(|name| name.as_str().ok_or_else(|| ApplicationError::invalid("name must be a string")))
    .transpose()?
    .map(str::to_owned);
  let descriptor = slot.get_descriptor();
  let mut provider = descriptor.pending_selection.map_or(descriptor.provider, |p| p.provider);
  if let Some(value) = input.get("provider") {
    provider = value
      .as_str()
      .ok_or_else(|| ApplicationError::invalid("provider must be a string"))?
      .to_owned();
    app.get_provider(&provider)?;
    if input.get("config").is_none() {
      return Err(ApplicationError::invalid("provider changes require config"));
    }
  }
  let config = input
    .get("config")
    .map(|v| {
      serde_json::from_value(v.clone()).map_err(|e| ApplicationError::invalid(e.to_string()))
    })
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
    Ok(slot.describe())
  })
  .await
}
/// Deletes a session or group: its storage, files and record, and a session's place in its
/// groups.
pub async fn delete(app: &Arc<App>, id: &str) -> Result<(), ApplicationError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(id).await?;
  let mut session = slot.lock_idle().await.ok_or_else(|| {
    ApplicationError::conflict("interrupt and wait for the session before deleting")
  })?;
  slot.require_live()?;
  slot.tools.background.shutdown().await;
  if let Some(shell) = slot.tools.tasks() {
    shell.shutdown().await.map_err(ApplicationError::internal)?;
  }
  app.mcp.close_session(id);
  let (management, target, deleted) = (app.management.clone(), id.to_owned(), slot.clone());
  blocking(move || {
    session.delete()?;
    deleted.mark_deleted();
    management.delete(&target)?;
    Ok(())
  })
  .await?;
  app.sessions.lock().await.remove(id);
  let (owner, target) = (app.clone(), id.to_owned());
  blocking(move || owner.leave_groups(&target)).await?;
  for directory in [app.data_dir.shell(id), app.data_dir.blobs(id)] {
    match tokio::fs::remove_dir_all(directory).await {
      Ok(()) => {}
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
      Err(e) => return Err(ApplicationError::internal(e)),
    }
  }
  let _ = slot.events.send(json!({"type":"deleted"}));
  let _ = app.events.send(json!({"type":"session_deleted","id":id}));
  // The session is gone either way; this only hands its room back to the file system.
  let storage = app.storage.clone();
  if let Err(error) = blocking(move || storage.shrink().map_err(ApplicationError::internal)).await {
    eprintln!("shrink storage after deleting {id}: {}", error.message);
  }
  Ok(())
}
