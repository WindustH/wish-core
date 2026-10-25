//! HTTP adapters for the Copilot device login flow.
use crate::application::{app::App, copilot_login};
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, State},
};
use serde_json::Value;
use std::sync::Arc;
pub async fn start(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(copilot_login::start(app, id).await?))
}
pub async fn status(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(copilot_login::status(app, id).await?))
}
