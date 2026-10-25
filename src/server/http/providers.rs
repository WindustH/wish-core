//! HTTP adapters; domain operations live in the application layer.
use super::sse;
use crate::application::app::App;
use crate::application::provider_operations;
use crate::application::provider_operations::{CatalogQuery, CompactInput, DraftCatalog};
use crate::protocol::Request;
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, Query, State},
  response::Response,
};
use serde_json::Value;
use std::sync::Arc;

pub async fn list(State(app): State<Arc<App>>) -> Json<Value> {
  Json(provider_operations::list(app).await)
}
pub async fn get(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(provider_operations::get(app, id).await?))
}
pub async fn call(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(request): Json<Request>,
) -> Result<Response, ApiError> {
  let response = provider_operations::call(app.clone(), id, request).await?;
  Ok(sse::model_response(response, app.lifecycle.stop.clone()))
}
pub async fn count(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(provider_operations::count(app, id, request).await?))
}
pub async fn compact(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<CompactInput>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(provider_operations::compact(app, id, input).await?))
}
pub async fn models(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<CatalogQuery>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(provider_operations::models(app, id, query).await?))
}
pub async fn draft_models(
  State(app): State<Arc<App>>,
  Json(draft): Json<DraftCatalog>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(provider_operations::draft_models(app, draft).await?))
}
pub async fn account(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(provider_operations::account(app, id).await?))
}
