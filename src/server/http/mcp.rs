//! HTTP adapters; domain operations live in the application layer.
use crate::application::app::App;
use crate::application::mcp_access;
use crate::application::mcp_access::{CallInput, ServersQuery, ToolQuery};
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, Query, State},
  http::HeaderMap,
};
use serde_json::Value;
use std::sync::Arc;

pub async fn servers(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<ServersQuery>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(mcp_access::servers(app, id, query, super::bridge::bearer_token(&headers)).await?))
}
pub async fn tool(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<ToolQuery>,
  headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(mcp_access::tool(app, id, query, super::bridge::bearer_token(&headers)).await?))
}
pub async fn call(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
  Json(input): Json<CallInput>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(mcp_access::call(app, id, super::bridge::bearer_token(&headers), input).await?))
}
pub async fn list(State(app): State<Arc<App>>) -> Json<Value> {
  Json(mcp_access::list(app).await)
}
pub async fn check(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(mcp_access::check(app, id).await?))
}
