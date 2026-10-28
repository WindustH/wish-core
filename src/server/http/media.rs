use crate::application::{app::App, media_access};
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, State},
};
use serde_json::Value;
use std::sync::Arc;
pub async fn status(
  State(app): State<Arc<App>>,
  Path((session, kind, job)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(media_access::status(app, session, kind, job).await?))
}
