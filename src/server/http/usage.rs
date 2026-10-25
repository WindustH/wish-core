//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::usage_queries;
use crate::application::usage_queries::{Daily, Range, Series};
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, Query, State},
};
use serde_json::Value;
use std::sync::Arc;

pub async fn global_usage(
  State(app): State<Arc<App>>,
  Query(range): Query<Range>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(usage_queries::global_usage(app, range).await?))
}
pub async fn session_usage(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(range): Query<Range>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(usage_queries::session_usage(app, id, range).await?))
}
pub async fn global_series(
  State(app): State<Arc<App>>,
  Query(query): Query<Series>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(usage_queries::global_series(app, query).await?))
}
pub async fn session_series(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<Series>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(usage_queries::session_series(app, id, query).await?))
}
pub async fn global_daily(
  State(app): State<Arc<App>>,
  Query(query): Query<Daily>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(usage_queries::global_daily(app, query).await?))
}
pub async fn session_daily(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<Daily>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(usage_queries::session_daily(app, id, query).await?))
}
