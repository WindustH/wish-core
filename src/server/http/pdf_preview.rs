use crate::application::{app::App, blob_preview::pdf};
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
}

async fn response(
  app: &Arc<App>,
  owner: &str,
  blob: &str,
  page: Option<u32>,
  request: HeaderMap,
) -> Result<Response, ApiError> {
  let source = pdf::prepare(app, owner, blob, page).await?;
  let mut headers = HeaderMap::new();
  headers.insert(
    header::CONTENT_TYPE,
    if page.is_some() { "application/pdf" } else { "application/json" }.parse().unwrap(),
  );
  headers.insert(header::CACHE_CONTROL, "private, no-cache".parse().unwrap());
  headers.insert(header::VARY, "Authorization".parse().unwrap());
  headers.insert(header::ETAG, source.etag.parse().unwrap());
  if super::conditional::not_modified(&request, &source.etag) {
    return Ok((StatusCode::NOT_MODIFIED, headers, Body::empty()).into_response());
  }
  let result = pdf::extract(app, source, page).await?;
  Ok((headers, Body::from(result.bytes)).into_response())
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
  response(&app, &owner, &blob, Some(query.page), headers).await
}
