//! HTTP adapters for task management.
use crate::{
  application::{
    app::App,
    task_access::{self, TaskRequest},
  },
  server::error::ApiError,
};
use axum::{
  Json,
  extract::{Path, State},
  http::HeaderMap,
};
use serde_json::Value;
use std::sync::Arc;
pub async fn session(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
  Json(request): Json<Value>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    task_access::as_session(
      app,
      id,
      super::bridge::bearer_token(&headers),
      task_access::parse_request(&request)?,
    )
    .await?,
  ))
}
pub async fn user(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(request): Json<Value>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(task_access::as_user(app, id, task_access::parse_request(&request)?).await?))
}
pub async fn list(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(task_access::as_user(app, id, TaskRequest::List).await?))
}

pub async fn background_foreground(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(task_access::as_user(app, id, TaskRequest::BackgroundForeground).await?))
}
