use crate::application::{app::App, blob_preview::image};
use crate::server::error::ApiError;
use axum::{
  body::Body,
  extract::{Path, Query, State},
  http::{HeaderMap, StatusCode, header},
  response::{IntoResponse, Response},
};
use std::sync::Arc;
#[derive(serde::Deserialize)]
pub struct Size {
  width: u32,
}
pub async fn preview(
  State(app): State<Arc<App>>,
  Path((owner, blob)): Path<(String, String)>,
  Query(size): Query<Size>,
  request: HeaderMap,
) -> Result<Response, ApiError> {
  let source = image::prepare(&app, &owner, &blob, size.width).await?;
  let mut headers = HeaderMap::new();
  headers.insert(header::CONTENT_TYPE, "image/webp".parse().unwrap());
  headers.insert(header::CACHE_CONTROL, "private, no-cache".parse().unwrap());
  headers.insert(header::VARY, "Authorization".parse().unwrap());
  headers.insert(header::ETAG, source.etag.parse().unwrap());
  if super::conditional::not_modified(&request, &source.etag) {
    return Ok((StatusCode::NOT_MODIFIED, headers, Body::empty()).into_response());
  }
  let result = image::render(&app, source, size.width).await?;
  Ok((headers, Body::from(result.bytes)).into_response())
}
