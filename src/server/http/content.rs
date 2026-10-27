//! Attachment and user-input HTTP adapters.
use crate::application::{
  app::App,
  attachments::{self, Blob, BlobMetadata},
  input::Input,
  session_commands,
};
use crate::server::error::ApiError;
use axum::{
  Json,
  body::Bytes,
  extract::{Path, State},
  http::{HeaderMap, StatusCode, header},
};
use serde_json::Value;
use std::sync::Arc;
pub async fn upload(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  body: Bytes,
) -> Result<Json<Blob>, ApiError> {
  Ok(Json(attachments::upload(&app, &id, &body).await?))
}
pub async fn download(
  State(app): State<Arc<App>>,
  Path((id, blob)): Path<(String, String)>,
) -> Result<(HeaderMap, Vec<u8>), ApiError> {
  let body = attachments::download(&app, &id, &blob).await?;
  let mut headers = HeaderMap::new();
  headers.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
  headers
    .insert(header::HeaderName::from_static("x-content-type-options"), "nosniff".parse().unwrap());
  Ok((headers, body))
}
pub async fn metadata(
  State(app): State<Arc<App>>,
  Path((id, blob)): Path<(String, String)>,
) -> Result<Json<BlobMetadata>, ApiError> {
  Ok(Json(attachments::metadata(&app, &id, &blob).await?))
}
pub async fn input(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Input>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((StatusCode::ACCEPTED, Json(session_commands::submit_input(app, id, input).await?)))
}

pub async fn upload_draft(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  body: Bytes,
) -> Result<Json<Blob>, ApiError> {
  Ok(Json(attachments::upload_draft(&app, &id, &body).await?))
}
