//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::history_queries;
use crate::application::history_queries::{Around, HistoryQuery, Search, Timeline};
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, Query, State},
};
use serde_json::Value;
use std::sync::Arc;

pub async fn timeline(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<Timeline>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(history_queries::timeline(app, id, query).await?))
}
pub async fn query(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(query): Json<HistoryQuery>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(history_queries::query(app, id, query).await?))
}
pub async fn search(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(query): Json<Search>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(history_queries::search(app, id, query).await?))
}
pub async fn read(
  State(app): State<Arc<App>>,
  Path((id, sequence)): Path<(String, u64)>,
  Query(around): Query<Around>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(history_queries::read(app, (id, sequence), around).await?))
}
