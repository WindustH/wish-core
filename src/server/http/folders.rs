//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::folder_access;
use crate::application::folder_access::{Move, NewFolder, Pin, Rename};
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, State},
  http::StatusCode,
};
use serde_json::Value;
use std::sync::Arc;

pub async fn list(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  Ok(Json(folder_access::list(app).await?))
}
pub async fn create(
  State(app): State<Arc<App>>,
  Json(input): Json<NewFolder>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((StatusCode::CREATED, Json(folder_access::create(app, input).await?)))
}
pub async fn rename(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Rename>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(folder_access::rename(app, id, input).await?))
}
pub async fn delete(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
  folder_access::delete(app, id).await?;
  Ok(StatusCode::NO_CONTENT)
}
pub async fn move_entries(
  State(app): State<Arc<App>>,
  Json(input): Json<Move>,
) -> Result<StatusCode, ApiError> {
  folder_access::move_entries(app, input).await?;
  Ok(StatusCode::NO_CONTENT)
}
pub async fn pin(
  State(app): State<Arc<App>>,
  Json(input): Json<Pin>,
) -> Result<StatusCode, ApiError> {
  folder_access::pin(app, input).await?;
  Ok(StatusCode::NO_CONTENT)
}
