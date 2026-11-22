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
  if metadata.as_ref().is_some_and(|value| value.to_string().len() > 64 * 1024) {
    return Err(ApplicationError::invalid("session metadata exceeds 64 KiB"));
  }
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
/// Deletes a session's storage, owned files, scratch directory and index record.
pub async fn delete(app: &Arc<App>, id: &str) -> Result<(), ApplicationError> {
  let owner = app.clone();
  let id = id.to_owned();
  app
    .lifecycle
    .while_open(|| {
      app.lifecycle.tasks.spawn(async move { release(&owner, &id, None).await.map(|_| ()) })
    })
    .ok_or_else(ApplicationError::shutting_down)?
    .await
    .map_err(ApplicationError::internal)?
}

pub(super) async fn release(
  app: &Arc<App>,
  id: &str,
  package: Option<(bool, super::archives::Include)>,
) -> Result<Value, ApplicationError> {
  let _draft = super::draft_uploads::lock_owner(app, id).await;
  let _opening = app.lock_session_open(id).await;
  let slot = app.cached_session(id).await;
  let _edit =
    if let Some(slot) = &slot { Some(slot.update_lock.clone().lock_owned().await) } else { None };
  let db = app.management.clone();
  let target = id.to_owned();
  let record = blocking(move || Ok(db.read(&target)?)).await?;
  let temp: Option<std::path::PathBuf> =
    serde_json::from_value(record["session"]["temp_dir"].clone())
      .map_err(ApplicationError::internal)?;
  if let Some(path) = &temp {
    super::session_temp::validate(id, path)?;
  }
  let mut idle = None;
  let mut publication = None;
  if let Some(slot) = &slot {
    slot.require_live()?;
    idle = Some(slot.lock_idle().await.ok_or_else(|| {
      ApplicationError::conflict("interrupt and wait for the session before deleting or packaging")
    })?);
    publication = Some(
      slot
        .blob_publication
        .clone()
        .try_write_owned()
        .map_err(|_| ApplicationError::conflict("files are being added to this session"))?,
    );
    if package.is_some_and(|(export, _)| !export)
      && (slot.tools.background.is_active()
        || !slot.tools.browser.can_unload()
        || slot.tools.tasks().is_some_and(|tasks| tasks.has_live_processes()))
    {
      return Err(ApplicationError::conflict(
        "close the terminal or browser and wait for running tools before archiving",
      ));
    }
  }
  let mut summary = Value::Null;
  if let Some((export, options)) = package {
    let app = app.clone();
    let owner = id.to_owned();
    let journal = crate::storage::management::archives::PendingRelease { temp_dir: temp.clone() };
    let hot = slot.clone();
    let session = idle.take();
    let packaged = crate::utils::blocking::cpu(move || {
      let sender = session.as_ref().map(|session| session.create_sender());
      let _frozen = sender.as_ref().map(|sender| sender.freeze_publication()).transpose()?;
      let summary = super::archives::session::pack(&app, &owner, record, options, export)?;
      let commit = || super::archives::persist(&app, &summary, (!export).then_some(&journal));
      if !export && let Some(slot) = hot {
        slot.commit_archive(commit)?;
      } else {
        commit()?;
      }
      Ok::<_, ApplicationError>((summary, session))
    })
    .await
    .map_err(ApplicationError::internal)??;
    idle = packaged.1;
    summary = packaged.0;
    if export {
      return Ok(summary);
    }
  }
  let target = id.to_owned();
  let journal = crate::storage::management::archives::PendingRelease { temp_dir: temp.clone() };
  if package.is_none() {
    let db = app.management.clone();
    let owner = target.clone();
    let hot = slot.clone();
    blocking(move || {
      let commit = || Ok(db.begin_delete(&owner, &journal)?);
      if let Some(slot) = hot { slot.commit_archive(commit) } else { commit() }
    })
    .await?;
  }
  // Active writers were refused before packaging. Retire the idle managers only once
  // the catalog has committed, so failed packages leave the live session usable.
  let shutdown = if let Some(slot) = &slot {
    let shell = slot.tools.tasks();
    let (_, _, result) = tokio::join!(
      slot.tools.background.shutdown(),
      slot.tools.browser.set_enabled(false),
      async {
        if let Some(shell) = shell {
          shell.shutdown().await?;
        }
        Ok::<_, crate::task::TaskError>(())
      }
    );
    result
  } else {
    Ok(())
  };
  app.mcp.close_session(id);
  app.previews.revoke(id, None);
  let storage = app.storage.clone();
  let deletion = blocking(move || {
    shutdown?;
    Ok(if let Some(mut session) = idle {
      session.delete()?
    } else {
      Session::delete_closed(&storage, &target)?
    })
  })
  .await;
  app.sessions.lock().await.remove(id);
  if let Some(slot) = slot {
    let _ = slot.events.send(json!({"type":"deleted"}));
  }
  let archive = summary["archive_id"].as_str().map(str::to_owned);
  if archive.is_some() {
    let _ = app.events.send(json!({"type":"session_archived","id":id,"archive":summary}));
  }
  let _ = app.events.send(json!({"type":"session_deleted","id":id}));
  deletion?;
  detach_files(app, id, temp).await?;
  let db = app.management.clone();
  let owner = id.to_owned();
  blocking(move || {
    Ok(if let Some(id) = archive { db.finish_release(&id)? } else { db.finish_delete(&owner)? })
  })
  .await?;
  app.reclaim.notify_one();
  drop(publication);
  Ok(summary)
}
pub(super) async fn detach_files(
  app: &App,
  id: &str,
  temp: Option<std::path::PathBuf>,
) -> Result<(), ApplicationError> {
  if let Some(path) = &temp {
    super::session_temp::validate(id, path)?;
  }
  for dir in temp.into_iter().chain([app.data_dir.shell(id), app.data_dir.blobs(id)]) {
    super::session_reclaim::detach(app, &dir).await?;
  }
  Ok(())
}
