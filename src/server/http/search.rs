//! HTTP adapters for web service availability and connection checks.
use crate::application::{app::App, web_services};
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, State},
};
use serde_json::Value;
use std::sync::Arc;
pub async fn list(State(app): State<Arc<App>>) -> Json<Value> {
  Json(web_services::search_status(&app))
}
pub async fn fetch_list(State(app): State<Arc<App>>) -> Json<Value> {
  Json(web_services::fetch_status(&app))
}
pub async fn check(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(web_services::check_search(&app, &id).await?))
}
pub async fn fetch_check(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(web_services::check_fetch(&app, &id).await?))
}
