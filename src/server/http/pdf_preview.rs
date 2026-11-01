use crate::application::{app::App, pdf_preview};
use crate::server::error::ApiError;
use axum::{
  body::Body,
  extract::{Path, Query, State},
  http::{HeaderMap, StatusCode, header},
  response::{IntoResponse, Response},
};
use std::sync::Arc;

#[derive(serde::Deserialize)]
pub struct Page {
  page: u32,
  width: u32,
}

async fn response(
  app: &Arc<App>,
  owner: &str,
  blob: &str,
  page: Option<(u32, u32)>,
  request: HeaderMap,
) -> Result<Response, ApiError> {
  let result = pdf_preview::read(app, owner, blob, page).await?;
  let mut headers = HeaderMap::new();
  headers.insert(
    header::CONTENT_TYPE,
    if page.is_some() { "image/png" } else { "application/json" }.parse().unwrap(),
  );
  headers.insert(header::CACHE_CONTROL, "private, no-cache".parse().unwrap());
  headers.insert(header::VARY, "Authorization".parse().unwrap());
  headers.insert(header::ETAG, result.etag.parse().unwrap());
  if request.get(header::IF_NONE_MATCH).is_some_and(|value| value == result.etag.as_str()) {
    return Ok((StatusCode::NOT_MODIFIED, headers, Body::empty()).into_response());
  }
  let bytes = if page.is_none() {
    let mut info: serde_json::Value = serde_json::from_slice(&result.bytes)
      .map_err(crate::application::error::ApplicationError::internal)?;
    info["version"] = serde_json::Value::String(result.etag);
    serde_json::to_vec(&info).map_err(crate::application::error::ApplicationError::internal)?
  } else {
    result.bytes.as_ref().clone()
  };
  Ok((headers, Body::from(bytes)).into_response())
}
pub async fn info(
  State(app): State<Arc<App>>,
  Path((owner, blob)): Path<(String, String)>,
  headers: HeaderMap,
) -> Result<Response, ApiError> {
  response(&app, &owner, &blob, None, headers).await
}
pub async fn page(
  State(app): State<Arc<App>>,
  Path((owner, blob)): Path<(String, String)>,
  Query(query): Query<Page>,
  headers: HeaderMap,
) -> Result<Response, ApiError> {
  response(&app, &owner, &blob, Some((query.page, query.width)), headers).await
}
