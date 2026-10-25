//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::settings;
use crate::application::settings::SaveConfig;
use crate::server::error::ApiError;
use axum::{Json, extract::State};
use serde_json::Value;
use std::sync::Arc;

pub async fn get(State(app): State<Arc<App>>) -> Json<Value> {
  Json(settings::get(app).await)
}
pub async fn save(
  State(app): State<Arc<App>>,
  Json(input): Json<SaveConfig>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(settings::save(app, input).await?))
}
pub async fn defaults(State(app): State<Arc<App>>) -> Json<Value> {
  Json(settings::defaults(app).await)
}
pub async fn shells() -> Json<Value> {
  Json(settings::shells().await)
}
pub async fn proxy_environment() -> Json<Value> {
  Json(settings::proxy_environment().await)
}
