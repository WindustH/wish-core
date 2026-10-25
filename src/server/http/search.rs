//! The settings page's view of the search providers: which can search now, and a test search.
use crate::server::{app::App, error::ApiError};
use axum::{
  Json,
  extract::{Path, State},
};
use serde_json::{Value, json};
use std::sync::Arc;

pub async fn list(State(app): State<Arc<App>>) -> Json<Value> {
  let providers = app.get_search_providers();
  Json(json!({
    "providers": app.search.describe(&providers),
    "available": app.search.is_available(&providers),
  }))
}

pub async fn check(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  let providers = app.get_search_providers();
  Ok(Json(app.lifecycle.until_shutdown(app.search.check(&id, &providers)).await??))
}

pub async fn fetch_list(State(app): State<Arc<App>>) -> Json<Value> {
  Json(app.fetch.describe())
}
pub async fn fetch_check(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  let accounts = app.get_search_providers();
  let page = app
    .lifecycle
    .until_shutdown(app.fetch.fetch(
      "https://example.com",
      false,
      &uuid::Uuid::new_v4().to_string(),
      &accounts,
      None,
      Some(&id),
    ))
    .await?
    .map_err(ApiError::bad_request)?;
  Ok(Json(json!(page)))
}
