//! HTTP adapter for session capability commands.
use crate::{
  application::{app::App, tool_commands},
  server::error::ApiError,
};
use axum::{
  Json,
  extract::{Path, State},
  http::HeaderMap,
};
use serde_json::Value;
use std::sync::Arc;
pub async fn execute(
  State(app): State<Arc<App>>,
  Path((id, name)): Path<(String, String)>,
  headers: HeaderMap,
  Json(arguments): Json<Value>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    tool_commands::execute(app, id, super::bridge::bearer_token(&headers), name, Some(arguments))
      .await?,
  ))
}
pub async fn describe(
  State(app): State<Arc<App>>,
  Path((id, name)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    tool_commands::execute(app, id, super::bridge::bearer_token(&headers), name, None).await?,
  ))
}
