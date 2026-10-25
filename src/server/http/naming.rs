//! HTTP adapter for an explicit naming operation.
use crate::{
  application::{app::App, naming},
  server::error::ApiError,
};
use axum::{
  Json,
  extract::{Path, State},
};
pub(crate) use naming::NamingInput;
use serde_json::Value;
use std::sync::Arc;
pub async fn generate(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<NamingInput>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(naming::generate(app, id, input).await?))
}
