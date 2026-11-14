//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::peer_commands::{Changes, NewSession, Send};
use crate::application::session_access;
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, State},
  http::{HeaderMap, StatusCode},
};
use serde_json::Value;
use std::sync::Arc;

pub async fn list(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_access::list(app, id, super::bridge::bearer_token(&headers)).await?))
}
pub async fn show(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(session_access::show(app, (id, target), super::bridge::bearer_token(&headers)).await?))
}
pub async fn create(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
  Json(input): Json<NewSession>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((
    StatusCode::CREATED,
    Json(session_access::create(app, id, super::bridge::bearer_token(&headers), input).await?),
  ))
}
pub async fn configure(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
  Json(changes): Json<Changes>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    session_access::configure(app, (id, target), super::bridge::bearer_token(&headers), changes)
      .await?,
  ))
}
pub async fn delete(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
  session_access::delete(app, (id, target), super::bridge::bearer_token(&headers)).await?;
  Ok(StatusCode::NO_CONTENT)
}
pub async fn send(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
  Json(input): Json<Send>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    session_access::send(app, (id, target), super::bridge::bearer_token(&headers), input).await?,
  ))
}
