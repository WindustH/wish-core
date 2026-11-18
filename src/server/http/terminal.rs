//! Authenticated user PTY transport; inherits the API bearer and same-origin middleware.
use crate::{
  application::{app::App, terminal_access},
  server::error::ApiError,
  task::TerminalRequest,
};
use axum::{
  Json,
  extract::{Path, State},
};
use serde_json::Value;
use std::sync::Arc;
pub async fn request(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(request): Json<TerminalRequest>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(terminal_access::execute(app, id, request).await?))
}
