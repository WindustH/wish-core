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
  let result = image::read(&app, &owner, &blob, size.width).await?;
  let mut headers = HeaderMap::new();
  headers.insert(header::CONTENT_TYPE, "image/webp".parse().unwrap());
  headers.insert(header::CACHE_CONTROL, "private, no-cache".parse().unwrap());
  headers.insert(header::VARY, "Authorization".parse().unwrap());
  headers.insert(header::ETAG, result.etag.parse().unwrap());
  if request.get(header::IF_NONE_MATCH).is_some_and(|value| value == result.etag.as_str()) {
    return Ok((StatusCode::NOT_MODIFIED, headers, Body::empty()).into_response());
  }
  Ok((headers, Body::from(result.bytes.as_ref().clone())).into_response())
}
