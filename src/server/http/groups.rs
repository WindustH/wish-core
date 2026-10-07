//! Groups over HTTP, for the user: making, renaming, re-membering and deleting one, its
//! transcript, posting to it, and the groups a session is in. Files posted to a group go through
//! `content`'s blob routes under `/groups/{id}/blobs`.
use super::content::input::Input;
use crate::server::{app::App, error::ApiError, groups::Author};
use axum::{
  Json,
  extract::{Path, Query, State},
  http::StatusCode,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewGroup {
  #[serde(default)]
  name: String,
  /// The sessions in it, besides the user.
  members: Vec<String>,
  /// The folder it goes in; none for the root of the list.
  #[serde(default)]
  folder: Option<String>,
}
pub async fn create(
  State(app): State<Arc<App>>,
  Json(input): Json<NewGroup>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  let group = app.create_group(input.name, input.members, None, input.folder).await?;
  Ok((StatusCode::CREATED, Json(json!(group))))
}
pub async fn get(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(json!(app.get_group(&id)?)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Changes {
  name: Option<String>,
  members: Option<Vec<String>>,
}
pub async fn update(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(changes): Json<Changes>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(json!(app.update_group(&id, changes.name, changes.members)?)))
}
pub async fn delete(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
  app.delete_group(&id).await?;
  Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Page {
  /// Messages before this one; the newest when absent.
  before: Option<u64>,
  limit: usize,
  /// Words a message's text must hold.
  query: String,
}
impl Default for Page {
  fn default() -> Self {
    Self { before: None, limit: 50, query: String::new() }
  }
}
/// A page of a group's transcript, newest first; `next` continues it.
pub async fn messages(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  app.get_group(&id)?;
  if page.limit == 0 {
    return Err(ApiError::bad_request("limit must be positive"));
  }
  let mut items = app.management.group_messages(&id, page.before, page.limit, &page.query)?;
  let more = items.len() > page.limit;
  items.truncate(page.limit);
  let next = more.then(|| items.last().map(|message| message.seq)).flatten();
  Ok(Json(json!({"items": items, "next": next})))
}
/// The user's post: text, with images and files uploaded to the group first.
pub async fn post(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Input>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  let group = app.get_group(&id)?;
  let posted = app.post(&group, Author::User, input.text.clone(), Some(input)).await?;
  Ok((StatusCode::CREATED, Json(json!({"message": posted.message, "woken": posted.woken}))))
}

/// The groups a session is in.
pub async fn of_session(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  app.get_session(&id).await?;
  Ok(Json(json!(app.management.groups_of(&id)?)))
}
