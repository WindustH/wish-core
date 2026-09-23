use crate::server::{app::App, error::ApiError};
use axum::{
  Json,
  extract::{Path, Query, State},
  response::{
    IntoResponse, Response, Sse,
    sse::{Event, KeepAlive},
  },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc};
use crate::{
  executor::model::CallResponse,
  protocol::{Request, UpstreamCompactionRequest, model_list::ModelListQuery},
};

pub async fn list(State(app): State<Arc<App>>) -> Json<Value> {
  Json(
    json!({"items":app.providers.read().unwrap().iter().map(|(id,p)|p.describe(id)).collect::<Vec<_>>()}),
  )
}
pub async fn get(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(app.get_provider(&id)?.describe(&id)))
}
pub async fn call(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(request): Json<Request>,
) -> Result<Response, ApiError> {
  app.require_open()?;
  let provider = app.get_provider(&id)?;
  let result = tokio::select! {
    _=app.stop.cancelled()=>return Err(ApiError::conflict("server is shutting down")),
    result=provider.client.call(&request)=>result?,
  };
  match result {
    CallResponse::Complete(response) => Ok(Json(response).into_response()),
    CallResponse::Stream(stream) => {
      let stream = futures_util::stream::unfold(
        (stream, app.stop.clone(), false),
        |(mut stream, stop, done)| async move {
          if done {
            return None;
          }
          let result = tokio::select! {
            _=stop.cancelled()=>Err("server is shutting down".to_owned()),
            result=stream.next()=>result.map_err(|e|e.to_string()),
          };
          let (event, done) = match result {
            Ok(Some(event)) => (
              Event::default().event("model_event").data(serde_json::to_string(&event).unwrap()),
              false,
            ),
            Ok(None) => (Event::default().event("done").data("{}"), true),
            Err(error) => {
              (Event::default().event("error").data(json!({"message":error}).to_string()), true)
            }
          };
          Some((Ok::<_, Infallible>(event), (stream, stop, done)))
        },
      );
      Ok(Sse::new(stream).keep_alive(KeepAlive::default()).into_response())
    }
  }
}
pub async fn count(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(request): Json<Request>,
) -> Result<Json<Value>, ApiError> {
  let provider = app.get_provider(&id)?;
  tokio::select! {
    _=app.stop.cancelled()=>Err(ApiError::conflict("server is shutting down")),
    result=provider.client.count_tokens(&request)=>Ok(Json(json!(result?))),
  }
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
  let request = UpstreamCompactionRequest { model: input.model, conversation: input.conversation };
  tokio::select! {
    _=app.stop.cancelled()=>Err(ApiError::conflict("server is shutting down")),
    result=provider.client.compact_upstream(&request)=>Ok(Json(json!(result?))),
  }
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
  page.unauthenticated = matches!(provider.config.auth, crate::server::provider::Auth::None);
  page.cursor = query.cursor;
  if let Some(limit) = query.limit {
    page.page_size = limit;
  }
  let catalog = tokio::select! {
    _=app.stop.cancelled()=>return Err(ApiError::conflict("server is shutting down")),
    result=provider.client.get_model_list(&page)=>result?,
  };
  Ok(Json(
    json!({"protocol":catalog.protocol.get_id(),"items":catalog.models.into_iter().map(|m|json!({
    "id":m.id,"name":m.name,"owner":m.owner,"created_at":m.created_at,"context_window":m.context_window,"max_output_tokens":m.max_output_tokens
  })).collect::<Vec<_>>(),"next_cursor":catalog.next_cursor,"warnings":catalog.warnings}),
  ))
}
pub async fn account(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  let provider = app.get_provider(&id)?;
  tokio::select! {
    _=app.stop.cancelled()=>Err(ApiError::conflict("server is shutting down")),
    result=provider.client.get_account_state(Some(&provider.config.base_url))=>Ok(Json(json!(result?))),
  }
}
