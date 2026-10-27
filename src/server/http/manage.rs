//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::session_commands;
use crate::application::session_commands::MoveInput;
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, State},
  http::{HeaderMap, StatusCode, header::IF_MATCH},
};
use serde_json::Value;
use std::sync::Arc;

pub async fn update_session(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
  Json(input): Json<Value>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    session_commands::update_session(
      app,
      id,
      headers
        .get(IF_MATCH)
        .map(|value| value.to_str().map(str::to_owned))
        .transpose()
        .map_err(|_| ApiError::invalid("If-Match must contain valid header text"))?,
      input,
    )
    .await?,
  ))
}
pub async fn delete_session(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
  session_commands::delete_session(app, id).await?;
  Ok(StatusCode::NO_CONTENT)
}
pub async fn cancel_input(
  State(app): State<Arc<App>>,
  Path((id, entry)): Path<(String, usize)>,
) -> Result<StatusCode, ApiError> {
  session_commands::cancel_input(app, (id, entry)).await?;
  Ok(StatusCode::NO_CONTENT)
}
pub async fn move_input(
  State(app): State<Arc<App>>,
  Path((id, entry)): Path<(String, usize)>,
  Json(input): Json<MoveInput>,
) -> Result<StatusCode, ApiError> {
  session_commands::move_input(app, (id, entry), input).await?;
  Ok(StatusCode::NO_CONTENT)
}
pub async fn clear_context(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_commands::clear_context(app, id).await?))
}
pub async fn fork(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((StatusCode::CREATED, Json(session_commands::fork(app, id).await?)))
}
