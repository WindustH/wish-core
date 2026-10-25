//! Folder organization and list ordering, with change notifications.
use crate::application::app::App;
use crate::application::error::ApplicationError;
use crate::storage::management::FolderRecord;
use crate::utils::time::Timestamp;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

fn announce(app: &App) {
  let _ = app.events.send(json!({"type": "list_changed"}));
}
fn named(name: String) -> Result<String, ApplicationError> {
  let name = name.trim().to_owned();
  if name.is_empty() {
    return Err(ApplicationError::invalid("a folder needs a name"));
  }
  Ok(name)
}

/// Every folder, by name.
pub async fn list(app: Arc<App>) -> Result<Value, ApplicationError> {
  Ok(json!(app.management.folders()?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewFolder {
  name: String,
  /// The folder it goes in; none for the root.
  #[serde(default)]
  parent: Option<String>,
}
pub async fn create(app: Arc<App>, input: NewFolder) -> Result<Value, ApplicationError> {
  let folder = FolderRecord {
    id: uuid::Uuid::new_v4().to_string(),
    name: named(input.name)?,
    parent: input.parent,
    pinned: false,
    created_at: Timestamp::now().0,
  };
  app.management.create_folder(&folder)?;
  announce(&app);
  Ok(json!(folder))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rename {
  name: String,
}
pub async fn rename(app: Arc<App>, id: String, input: Rename) -> Result<Value, ApplicationError> {
  app.management.rename_folder(&id, &named(input.name)?)?;
  announce(&app);
  Ok(json!(app.management.folder(&id)?.ok_or_else(ApplicationError::not_found)?))
}
/// Deletes a folder; what it held goes to the folder it was in.
pub async fn delete(app: Arc<App>, id: String) -> Result<(), ApplicationError> {
  app.management.delete_folder(&id)?;
  announce(&app);
  Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Move {
  /// Sessions, groups and folders.
  ids: Vec<String>,
  /// Where they go; none for the root.
  folder: Option<String>,
}
pub async fn move_entries(app: Arc<App>, input: Move) -> Result<(), ApplicationError> {
  app.management.place(&input.ids, input.folder.as_deref())?;
  announce(&app);
  Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
  /// Sessions, groups and folders.
  ids: Vec<String>,
  pinned: bool,
}
pub async fn pin(app: Arc<App>, input: Pin) -> Result<(), ApplicationError> {
  app.management.pin(&input.ids, input.pinned)?;
  announce(&app);
  Ok(())
}
