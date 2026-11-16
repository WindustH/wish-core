//! HTTP adapters for linked-folder navigation and conversation bookmarks.
use crate::{
  application::{app::App, library_items as items},
  server::error::ApiError,
  storage::blobs::library::{Folder, bookmarks::Bookmark, linked_folders},
};
use axum::{
  Json,
  extract::{Path, Query, State},
};
use std::sync::Arc;

pub async fn link_path(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<items::LinkPath>,
) -> Result<Json<items::LinkedItem>, ApiError> {
  Ok(Json(items::link_path(&app, &id, input).await?))
}
pub async fn link(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<linked_folders::Link>,
) -> Result<Json<Folder>, ApiError> {
  Ok(Json(items::link_folder(&app, &id, input).await?))
}
pub async fn browse(
  State(app): State<Arc<App>>,
  Path((id, folder)): Path<(String, String)>,
  Query(query): Query<items::Browse>,
) -> Result<Json<linked_folders::Page>, ApiError> {
  Ok(Json(items::browse(&app, &id, folder, query).await?))
}
pub async fn rename_child(
  State(app): State<Arc<App>>,
  Path((id, folder)): Path<(String, String)>,
  Json(input): Json<items::ChildName>,
) -> Result<Json<serde_json::Value>, ApiError> {
  items::rename_child(&app, &id, folder, input).await?;
  Ok(Json(serde_json::json!({"updated":true})))
}
pub async fn open_child(
  State(app): State<Arc<App>>,
  Path((id, folder)): Path<(String, String)>,
  Json(input): Json<items::ChildPath>,
) -> Result<Json<crate::storage::blobs::Blob>, ApiError> {
  Ok(Json(items::open_child(&app, &id, folder, input).await?))
}
pub async fn bookmarks(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Vec<Bookmark>>, ApiError> {
  Ok(Json(items::list_bookmarks(&app, &id).await?))
}
pub async fn add_bookmark(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<items::BookmarkInput>,
) -> Result<Json<Bookmark>, ApiError> {
  Ok(Json(items::add_bookmark(&app, &id, input).await?))
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Name {
  name: String,
}
pub async fn rename_bookmark(
  State(app): State<Arc<App>>,
  Path((id, bookmark)): Path<(String, String)>,
  Json(input): Json<Name>,
) -> Result<Json<serde_json::Value>, ApiError> {
  items::edit_bookmark(&app, &id, bookmark, Some(input.name)).await?;
  Ok(Json(serde_json::json!({"updated":true})))
}
pub async fn delete_bookmark(
  State(app): State<Arc<App>>,
  Path((id, bookmark)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
  items::edit_bookmark(&app, &id, bookmark, None).await?;
  Ok(Json(serde_json::json!({"deleted":true})))
}
