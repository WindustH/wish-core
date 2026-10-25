//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::skill_access;
use crate::application::skill_access::{ShowQuery, SkillsQuery};
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, Query, State},
  http::HeaderMap,
};
use serde_json::Value;
use std::sync::Arc;

pub async fn session_list(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<SkillsQuery>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(skill_access::session_list(app, id, query, super::bridge::bearer_token(&headers)).await?))
}
pub async fn session_show(
  State(app): State<Arc<App>>,
  Path((id, name)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(
    skill_access::session_show(app, (id, name), super::bridge::bearer_token(&headers)).await?,
  ))
}
pub async fn list(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  Ok(Json(skill_access::list(app).await?))
}
pub async fn show(
  State(app): State<Arc<App>>,
  Path(name): Path<String>,
  Query(query): Query<ShowQuery>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(skill_access::show(app, name, query).await?))
}
