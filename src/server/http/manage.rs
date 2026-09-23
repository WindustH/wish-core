use crate::server::{
  app::App,
  error::{ApiError, blocking},
  session::CreateSession,
};
use axum::{
  Json,
  extract::{Path, State},
  http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
use crate::{protocol::Message, session::EntryId};

pub async fn configuration(State(app): State<Arc<App>>) -> Json<Value> {
  Json(app.configuration.lock().await.describe())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveConfig {
  revision: String,
  config: Value,
}
pub async fn save_configuration(
  State(app): State<Arc<App>>,
  Json(input): Json<SaveConfig>,
) -> Result<Json<Value>, ApiError> {
  app.require_open()?;
  Ok(Json(app.save_configuration(input.revision, input.config).await?))
}
pub async fn update_session(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
  Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
  app.require_open()?;
  let slot = app.get_session(&id).await?;
  let _edit = slot.selection_edit.clone().lock_owned().await;
  // Names belong to the descriptor, not the executing session. Do not wait
  // for the execution mutex or disturb a pending model selection.
  if input.as_object().is_some_and(|object| object.len() == 1 && object.contains_key("name")) {
    let name = input["name"].as_str()
      .ok_or_else(|| ApiError::bad_request("name must be a string"))?.to_owned();
    return blocking(move || {
      slot.require_live()?;
      let mut descriptor = slot.descriptor.write().unwrap();
      if let Some(revision) = headers.get("if-match") {
        if revision.to_str().ok() != Some(descriptor.revision.to_string().as_str()) {
          return Err(ApiError::conflict("session changed; reload before saving"));
        }
      }
      let mut next = descriptor.clone();
      next.name = name;
      next.revision += 1;
      next.updated_at = crate::session::statistics::Timestamp::now().0;
      let status = slot.status.lock().unwrap().clone();
      slot.index.save(&json!({"session":next,"status":status}))?;
      *descriptor = next;
      drop(descriptor);
      slot.persist_index()?;
      Ok(Json(slot.describe()))
    }).await;
  }
  let session = slot.session.clone().try_lock_owned();
  if session.is_err() {
    let object = input.as_object().ok_or_else(|| ApiError::bad_request("expected an object"))?;
    if object.keys().any(|key| !["provider", "config"].contains(&key.as_str())) {
      return Err(ApiError::conflict("only model and reasoning settings can change while running"));
    }
    let config: crate::session::SessionConfig = serde_json::from_value(
      input.get("config").cloned().ok_or_else(|| ApiError::bad_request("config required"))?
    ).map_err(ApiError::internal)?;
    let config = slot.configure_tools(config)?;
    return blocking(move || {
      slot.require_live()?;
      let mut descriptor = slot.descriptor.write().unwrap();
      if let Some(revision) = headers.get("if-match") {
        if revision.to_str().ok() != Some(descriptor.revision.to_string().as_str()) {
          return Err(ApiError::conflict("session changed; reload before saving"));
        }
      }
      let mut current = descriptor.pending_selection.as_ref().map(|p| json!(p.config))
        .unwrap_or_else(|| slot.status.lock().unwrap()["config"].clone());
      let mut desired = json!(config);
      for key in ["model", "reasoning", "max_output_tokens"] {
        current.as_object_mut().unwrap().remove(key);
        desired.as_object_mut().unwrap().remove(key);
      }
      if current != desired { return Err(ApiError::conflict("only model, reasoning and output limit can change while running")); }
      let provider = input.get("provider").map(|v| v.as_str().ok_or_else(|| ApiError::bad_request("provider must be a string"))).transpose()?
        .unwrap_or_else(|| descriptor.pending_selection.as_ref().map(|p| p.provider.as_str()).unwrap_or(&descriptor.provider)).to_owned();
      app.get_provider(&provider)?;
      let mut next = descriptor.clone();
      next.pending_selection = Some(crate::server::session::selection::PendingSelection { provider, config });
      next.revision += 1;
      next.updated_at = crate::session::statistics::Timestamp::now().0;
      let status = slot.status.lock().unwrap().clone();
      slot.index.save(&json!({"session":next,"status":status}))?;
      *descriptor = next;
      drop(descriptor);
      slot.persist_index()?;
      Ok(Json(slot.describe()))
    }).await;
  }
  let mut session = session.unwrap();
  slot.require_live()?;
  let object = input.as_object().ok_or_else(|| ApiError::bad_request("expected an object"))?;
  for key in object.keys() {
    if !["name", "provider", "config", "metadata"].contains(&key.as_str()) {
      return Err(ApiError::bad_request(format!("unknown field: {key}")));
    }
  }
  let descriptor = slot.get_descriptor();
  if let Some(revision) = headers.get("if-match") {
    if revision.to_str().ok() != Some(descriptor.revision.to_string().as_str()) {
      return Err(ApiError::conflict("session changed; reload before saving"));
    }
  }
  let mut next = descriptor;
  if let Some(name) = input.get("name") {
    next.name =
      name.as_str().ok_or_else(|| ApiError::bad_request("name must be a string"))?.to_owned();
  }
  if let Some(provider) = input.get("provider") {
    next.provider = provider
      .as_str()
      .ok_or_else(|| ApiError::bad_request("provider must be a string"))?
      .to_owned();
    app.get_provider(&next.provider)?;
    if input.get("config").is_none() {
      return Err(ApiError::bad_request("provider changes require config"));
    }
  }
  let config = input
    .get("config")
    .map(|v| serde_json::from_value(v.clone()).map_err(|e| ApiError::bad_request(e.to_string())))
    .transpose()?;
  let config = config.map(|v| slot.configure_tools(v)).transpose()?;
  blocking(move || {
    if let Some(config) = config {
      next.pending_selection = None;
      session.set_config(config)?;
    }
    if let Some(metadata) = input.get("metadata") {
      session.set_metadata(metadata.clone())?;
    }
    next.revision += 1;
    next.updated_at = crate::session::statistics::Timestamp::now().0;
    *slot.descriptor.write().unwrap() = next;
    slot.update_snapshot(&session);
    slot.persist_index()?;
    Ok(Json(slot.describe()))
  })
  .await
}
pub async fn delete_session(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
  app.require_open()?;
  let slot = app.get_session(&id).await?;
  let mut session = slot
    .session
    .clone()
    .try_lock_owned()
    .map_err(|_| ApiError::conflict("interrupt and wait for the session before deleting"))?;
  slot.require_live()?;
  if let Some(shell) = &slot.tools.shell {
    shell.shutdown().await.map_err(ApiError::internal)?;
  }
  let index = app.index.clone();
  let target = id.clone();
  let owned = slot.clone();
  blocking(move || {
    session.delete()?;
    owned.deleted.store(true, Ordering::Release);
    index.delete(&target)?;
    Ok(())
  })
  .await?;
  app.sessions.lock().await.remove(&id);
  for directory in ["shell", "blobs"] {
    match tokio::fs::remove_dir_all(app.data_dir.join(directory).join(&id)).await {
      Ok(()) => {}
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
      Err(e) => return Err(ApiError::internal(e)),
    }
  }
  let _ = slot.events.send(json!({"type":"deleted"}));
  let _ = app.events.send(json!({"type":"session_deleted","id":id}));
  Ok(StatusCode::NO_CONTENT)
}
pub async fn cancel_input(
  State(app): State<Arc<App>>,
  Path((id, entry)): Path<(String, usize)>,
) -> Result<StatusCode, ApiError> {
  let slot = app.get_session(&id).await?;
  blocking(move || {
    slot.require_live()?;
    slot.handle.cancel_queued_input(EntryId(entry)).map_err(|error| match error {
      crate::session::SessionError::InvalidEntry(_) => {
        ApiError::conflict("input has already been consumed or cancelled")
      }
      error => error.into(),
    })?;
    slot.persist_index()?;
    Ok(StatusCode::NO_CONTENT)
  })
  .await
}

pub async fn clear_context(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  let slot = app.get_session(&id).await?;
  let mut session =
    slot.session.clone().try_lock_owned().map_err(|_| ApiError::conflict("session is running"))?;
  blocking(move || {
    slot.require_live()?;
    let generation = session.get_active_generation()?;
    let entries = session.get_generation_entries(generation.id)?;
    let mut prefix = Vec::new();
    let mut position = 0;
    while let Some(id) = entries.get(position)? {
      let entry = session.get_entry(*id)?.ok_or_else(ApiError::not_found)?;
      if !matches!(entry.message, Message::System { .. } | Message::Developer { .. }) {
        break;
      }
      prefix.push(*id);
      position += 1;
    }
    session.prepare_standby_generation(prefix)?;
    session.activate_standby_generation()?;
    slot.update_snapshot(&session);
    slot.persist_index()?;
    Ok(Json(slot.describe()))
  })
  .await
}
pub async fn fork(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  let slot = app.get_session(&id).await?;
  let session =
    slot.session.clone().try_lock_owned().map_err(|_| ApiError::conflict("session is running"))?;
  let descriptor = slot.get_descriptor();
  let mut config = session.get_config().clone();
  config.tools.clear();
  let request = session.build_request()?;
  let input = CreateSession {
    name: format!("{} (copy)", descriptor.name),
    provider: descriptor.provider,
    cwd: descriptor.cwd,
    shell: descriptor.shell,
    config,
    metadata: session.get_metadata().clone(),
    initial_messages: request.conversation,
  };
  drop(session);
  Ok((StatusCode::CREATED, Json(app.create_session(input).await?.describe())))
}
pub async fn defaults(State(app): State<Arc<App>>) -> Json<Value> {
  let config = app.configuration.lock().await;
  Json(
    json!({"defaults":config.config.defaults,"session_config":config.config.defaults.session_config()}),
  )
}
pub async fn events(
  State(app): State<Arc<App>>,
) -> axum::response::Sse<
  impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
  let receiver = app.events.subscribe();
  let stop = app.stop.clone();
  let stream = futures_util::stream::unfold(
    (receiver, stop, true),
    |(mut receiver, stop, first)| async move {
      let value = if first {
        json!({"type":"snapshot"})
      } else {
        tokio::select! {
          _=stop.cancelled()=>return None,
          result=receiver.recv()=>match result{Ok(v)=>v,Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>json!({"type":"gap"}),Err(_)=>return None}
        }
      };
      Some((
        Ok(axum::response::sse::Event::default().event("wish").data(value.to_string())),
        (receiver, stop, false),
      ))
    },
  );
  axum::response::Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
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
    slot.handle.move_queued_input(EntryId(entry), input.before.map(EntryId)).map_err(|error| {
      match error {
        crate::session::SessionError::InvalidEntry(_) => {
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
