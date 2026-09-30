//! Providers over HTTP: what is configured, their model catalogs and account readings, and direct
//! calls to a provider's client outside any session.
use super::sse;
use crate::protocol::{Request, UpstreamCompactionRequest, model_list::ModelListQuery};
use crate::server::{app::App, error::ApiError, provider::Auth};
use axum::{
  Json,
  extract::{Path, Query, State},
  response::Response,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub async fn list(State(app): State<Arc<App>>) -> Json<Value> {
  Json(json!({"items":app.get_providers().values().map(|p|p.describe()).collect::<Vec<_>>()}))
}
pub async fn get(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(app.get_provider(&id)?.describe()))
}
pub async fn call(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(request): Json<Request>,
) -> Result<Response, ApiError> {
  app.lifecycle.require_open()?;
  let provider = app.get_provider(&id)?;
  let response = app.lifecycle.until_shutdown(provider.client.call(&request)).await??;
  Ok(sse::model_response(response, app.lifecycle.stop.clone()))
}
pub async fn count(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
  let provider = app.get_provider(&id)?;
  let count = app.lifecycle.until_shutdown(provider.client.count_tokens(&request)).await??;
  Ok(Json(json!(count)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompactInput {
  pub model: String,
  pub conversation: Vec<crate::protocol::Message>,
}
pub async fn compact(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<CompactInput>,
) -> Result<Json<Value>, ApiError> {
  let provider = app.get_provider(&id)?;
  let request = UpstreamCompactionRequest {
    model: input.model,
    conversation: input.conversation,
    tools: Vec::new(),
    tool_choice: None,
    reasoning: None,
    cache: None,
  };
  let compaction =
    app.lifecycle.until_shutdown(provider.client.compact_upstream(&request)).await??;
  Ok(Json(json!(compaction)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogQuery {
  cursor: Option<String>,
  limit: Option<u32>,
}
pub async fn models(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<CatalogQuery>,
) -> Result<Json<Value>, ApiError> {
  let provider = app.get_provider(&id)?;
  let path = provider
    .config
    .model_list_path
    .as_deref()
    .ok_or_else(|| ApiError::bad_request("model_list_path is not configured"))?;
  let mut page = ModelListQuery::first(
    provider.config.model_list_base_url.as_deref().unwrap_or(&provider.config.base_url),
    path,
  );
  page.unauthenticated = matches!(provider.config.auth, Auth::None);
  page.cursor = query.cursor;
  if let Some(limit) = query.limit {
    page.page_size = limit;
  }
  // Cached while fresh, and one fetch in flight at a time; see the model_catalog module.
  let catalog =
    app.lifecycle.until_shutdown(provider.catalog.page(&provider.client, &page)).await??;
  let items: Vec<_> = catalog
    .models
    .into_iter()
    .map(|m| {
      json!({
        "id": m.id,
        "name": m.name,
        "owner": m.owner,
        "created_at": m.created_at,
        "context_window": m.context_window,
        "max_output_tokens": m.max_output_tokens,
      })
    })
    .collect();
  Ok(Json(json!({
    "protocol": catalog.protocol.get_id(),
    "items": items,
    "next_cursor": catalog.next_cursor,
    "warnings": catalog.warnings,
  })))
}
pub async fn account(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  let provider = app.get_provider(&id)?;
  // An account reading has a fixed path on its service's host. The provider's base URL is where
  // conversations go and often carries a path of its own, so it never stands in for that host.
  let host = provider.config.account_state_base_url.as_deref();
  let state = app.lifecycle.until_shutdown(provider.client.get_account_state(host)).await??;
  Ok(Json(json!(state)))
}
