//! How the user arranges the list, over HTTP: folders, and moving and pinning what the list holds,
//! sessions, groups and folders alike, as `mv` moves files and directories alike. Every change
//! announces `list_changed`.
use crate::server::{app::App, error::ApiError, management::FolderRecord};
use crate::utils::time::Timestamp;
use axum::{
  Json,
  extract::{Path, State},
  http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

fn announce(app: &App) {
  let _ = app.events.send(json!({"type": "list_changed"}));
}
fn named(name: String) -> Result<String, ApiError> {
  let name = name.trim().to_owned();
  if name.is_empty() {
    return Err(ApiError::bad_request("a folder needs a name"));
  }
  Ok(name)
}

/// Every folder, by name.
pub async fn list(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  Ok(Json(json!(app.management.folders()?)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewFolder {
  name: String,
  /// The folder it goes in; none for the root.
  #[serde(default)]
  parent: Option<String>,
}
pub async fn create(
  State(app): State<Arc<App>>,
  Json(input): Json<NewFolder>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  let folder = FolderRecord {
    id: uuid::Uuid::new_v4().to_string(),
    name: named(input.name)?,
    parent: input.parent,
    pinned: false,
    created_at: Timestamp::now().0,
  };
  app.management.create_folder(&folder)?;
  announce(&app);
  Ok((StatusCode::CREATED, Json(json!(folder))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rename {
  name: String,
}
pub async fn rename(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Rename>,
) -> Result<Json<Value>, ApiError> {
  app.management.rename_folder(&id, &named(input.name)?)?;
  announce(&app);
  Ok(Json(json!(app.management.folder(&id)?.ok_or_else(ApiError::not_found)?)))
}
/// Deletes a folder; what it held goes to the folder it was in.
pub async fn delete(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
  app.management.delete_folder(&id)?;
  announce(&app);
  Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Move {
  /// Sessions, groups and folders.
  ids: Vec<String>,
  /// Where they go; none for the root.
  folder: Option<String>,
}
pub async fn move_entries(
  State(app): State<Arc<App>>,
  Json(input): Json<Move>,
) -> Result<StatusCode, ApiError> {
  app.management.place(&input.ids, input.folder.as_deref())?;
  announce(&app);
  Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
  /// Sessions, groups and folders.
  ids: Vec<String>,
  pinned: bool,
}
pub async fn pin(
  State(app): State<Arc<App>>,
  Json(input): Json<Pin>,
) -> Result<StatusCode, ApiError> {
  app.management.pin(&input.ids, input.pinned)?;
  announce(&app);
  Ok(StatusCode::NO_CONTENT)
}
