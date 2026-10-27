//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::peer_commands::{AddMembers, Changes, NewGroup, NewSession, Send};
use crate::application::session_access as peer_commands;
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
  Ok(Json(peer_commands::list(app, id, super::bridge::bearer_token(&headers)).await?))
}
pub async fn show(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(peer_commands::show(app, (id, target), super::bridge::bearer_token(&headers)).await?))
}
pub async fn create(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
  Json(input): Json<NewSession>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((
    StatusCode::CREATED,
    Json(peer_commands::create(app, id, super::bridge::bearer_token(&headers), input).await?),
  ))
}
pub async fn configure(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
  Json(changes): Json<Changes>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    peer_commands::configure(app, (id, target), super::bridge::bearer_token(&headers), changes)
      .await?,
  ))
}
pub async fn delete(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
  peer_commands::delete(app, (id, target), super::bridge::bearer_token(&headers)).await?;
  Ok(StatusCode::NO_CONTENT)
}
pub async fn send(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
  Json(input): Json<Send>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    peer_commands::send(app, (id, target), super::bridge::bearer_token(&headers), input).await?,
  ))
}
pub async fn create_group(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
  Json(input): Json<NewGroup>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((
    StatusCode::CREATED,
    Json(peer_commands::create_group(app, id, super::bridge::bearer_token(&headers), input).await?),
  ))
}
pub async fn add_members(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
  Json(input): Json<AddMembers>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    peer_commands::add_members(app, (id, target), super::bridge::bearer_token(&headers), input)
      .await?,
  ))
}
pub async fn leave(
  State(app): State<Arc<App>>,
  Path((id, target)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
  peer_commands::leave(app, (id, target), super::bridge::bearer_token(&headers)).await?;
  Ok(StatusCode::NO_CONTENT)
}
