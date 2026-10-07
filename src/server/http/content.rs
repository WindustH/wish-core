//! Content from the page: blobs uploaded to a session or a group, and a session's user input - text
//! with attachments - which is queued and wakes it.
pub(crate) mod input;
use crate::server::{app::App, blobs, error::ApiError};
use axum::{
  Json,
  body::Bytes,
  extract::{Path, State},
  http::{HeaderMap, StatusCode, header},
};
use input::Input;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// Refuses an id that names neither a session nor a group: the owners of `blobs/<id>`.
async fn require_owner(app: &Arc<App>, id: &str) -> Result<(), ApiError> {
  if app.management.group(id)?.is_none() {
    app.get_session(id).await?;
  }
  Ok(())
}
pub async fn upload(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  body: Bytes,
) -> Result<Json<blobs::Blob>, ApiError> {
  app.lifecycle.require_open()?;
  require_owner(&app, &id).await?;
  Ok(Json(blobs::save_upload(&app.data_dir.blobs(&id), &body).await?))
}
pub async fn download(
  State(app): State<Arc<App>>,
  Path((id, blob)): Path<(String, String)>,
) -> Result<(HeaderMap, Vec<u8>), ApiError> {
  require_owner(&app, &id).await?;
  if !blobs::is_blob_id(&blob) {
    return Err(ApiError::not_found());
  }
  let body = blobs::read(&app.data_dir.blobs(&id), &blob).await?;
  let mut headers = HeaderMap::new();
  headers.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
  headers
    .insert(header::HeaderName::from_static("x-content-type-options"), "nosniff".parse().unwrap());
  Ok((headers, body))
}
#[derive(Serialize)]
pub struct BlobMetadata {
  id: String,
  mime_type: String,
  byte_count: usize,
}
pub async fn metadata(
  State(app): State<Arc<App>>,
  Path((id, blob)): Path<(String, String)>,
) -> Result<Json<BlobMetadata>, ApiError> {
  require_owner(&app, &id).await?;
  if !blobs::is_blob_id(&blob) {
    return Err(ApiError::not_found());
  }
  let item = blobs::read_description(&app.data_dir.blobs(&id), &blob).await?;
  Ok(Json(BlobMetadata { id: item.id, mime_type: item.mime_type, byte_count: item.byte_count }))
}
pub async fn input(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Input>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&id).await?;
  let message = input::message(&app.data_dir.blobs(&id), input).await?;
  let entry = slot.enqueue(message).await?;
  slot.touch()?;
  slot.schedule(&app);
  Ok((StatusCode::ACCEPTED, Json(json!({"entry":entry}))))
}
