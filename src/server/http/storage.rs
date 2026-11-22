//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::storage_cleanup::{self, PruneRequest};
use crate::server::error::ApiError;
use axum::{Json, extract::State};
use serde_json::Value;
use std::sync::Arc;

use crate::application::storage_usage;

pub async fn session_preview(
  State(app): State<Arc<App>>,
  axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(storage_usage::session_preview(app, id).await?))
}

pub async fn session_storage(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  Ok(Json(storage_usage::session_storage(app).await?))
}
pub async fn storage(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  Ok(Json(storage_usage::storage(app).await?))
}
pub async fn detail(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  Ok(Json(storage_usage::detail(app).await?))
}
#[derive(serde::Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct UsageOptions {
  pub(super) archive: bool,
}
pub async fn prune_usage(
  State(app): State<Arc<App>>,
  options: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    storage_usage::prune_usage(
      app,
      super::archives::optional_body::<UsageOptions>(&options)?.archive,
    )
    .await?,
  ))
}

pub async fn prune(
  State(app): State<Arc<App>>,
  Json(input): Json<PruneRequest>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(storage_cleanup::prune(app, input).await?))
}
