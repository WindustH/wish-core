use crate::{
  application::{
    app::App,
    archives::{self, Include},
  },
  server::error::ApiError,
  storage::management::archives::Filter,
};
use axum::{
  Json,
  extract::{Path, Query, State},
  http::{HeaderMap, StatusCode},
  response::Response,
};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) fn optional_body<T: serde::de::DeserializeOwned + Default>(
  body: &[u8],
) -> Result<T, ApiError> {
  if body.is_empty() {
    return Ok(T::default());
  }
  serde_json::from_slice(body).map_err(|e| ApiError::invalid(e.to_string()))
}
pub async fn archive(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(archives::package(&app, id, false, Include::default()).await?))
}
pub async fn export(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  input: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
  let m = archives::package(&app, id.clone(), true, optional_body(&input)?).await?;
  let key = m["archive_id"].as_str().ok_or_else(|| ApiError::internal("missing export ID"))?;
  let expires =
    m["expires_at"].as_u64().ok_or_else(|| ApiError::internal("missing export expiry"))?;
  let url = super::media_grant::package_url(&app, &format!("/exports/{key}"), expires / 1000);
  Ok(Json(
    json!({"export_id":key,"session_id":id,"filename":archives::filename(&m)?,"bytes":m["archive_bytes"],"expires_at":expires,"download_url":url}),
  ))
}
pub async fn list(
  State(app): State<Arc<App>>,
  Query(query): Query<Filter>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(archives::list(&app, query).await?))
}
pub async fn detail(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(archives::detail(&app, &id, false).await?))
}
pub async fn remove(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
  archives::remove(&app, id, false).await?;
  Ok(StatusCode::NO_CONTENT)
}
async fn file(
  app: Arc<App>,
  id: String,
  headers: HeaderMap,
  export: bool,
) -> Result<Response, ApiError> {
  let (opened, filename) = archives::download(&app, &id, export).await?;
  let mut response = super::content::serve_file(opened, headers).await?;
  response.headers_mut().insert(
    axum::http::header::CONTENT_DISPOSITION,
    format!("attachment; filename=\"{filename}\"").parse().map_err(ApiError::internal)?,
  );
  Ok(response)
}
pub async fn download(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
) -> Result<Response, ApiError> {
  file(app, id, headers, false).await
}
pub async fn download_export(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  headers: HeaderMap,
) -> Result<Response, ApiError> {
  file(app, id, headers, true).await
}

pub async fn download_grant(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  archives::detail(&app, &id, false).await?;
  let expires = crate::utils::time::unix_seconds() + 900;
  Ok(Json(
    json!({"download_url":super::media_grant::package_url(&app, &format!("/archives/{id}/download"), expires)}),
  ))
}
