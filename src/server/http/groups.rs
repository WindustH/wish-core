//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::group_access;
use crate::application::group_access::{Changes, NewGroup, Page};
use crate::application::input::Input;
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, Query, State},
  http::StatusCode,
};
use serde_json::Value;
use std::sync::Arc;

pub async fn create(
  State(app): State<Arc<App>>,
  Json(input): Json<NewGroup>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((StatusCode::CREATED, Json(group_access::create(app, input).await?)))
}
pub async fn get(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(group_access::get(app, id).await?))
}
pub async fn update(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(changes): Json<Changes>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(group_access::update(app, id, changes).await?))
}
pub async fn delete(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
  group_access::delete(app, id).await?;
  Ok(StatusCode::NO_CONTENT)
}
pub async fn messages(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(page): Query<Page>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(group_access::messages(app, id, page).await?))
}
pub async fn post(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Input>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((StatusCode::CREATED, Json(group_access::post(app, id, input).await?)))
}
pub async fn of_session(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(group_access::of_session(app, id).await?))
}
