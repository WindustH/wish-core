//! Attachment and user-input HTTP adapters.
use crate::application::{
  app::App,
  attachments::{self, Blob, BlobMetadata},
  input::Input,
  session_commands,
};
use crate::server::error::ApiError;
use axum::{
  Json,
  body::Bytes,
  extract::{Path, State},
  http::{HeaderMap, StatusCode, header},
};
use serde_json::Value;
use std::sync::Arc;
pub async fn upload(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  body: Bytes,
) -> Result<Json<Blob>, ApiError> {
  Ok(Json(attachments::upload(&app, &id, &body).await?))
}
pub async fn download(
  State(app): State<Arc<App>>,
  Path((id, blob)): Path<(String, String)>,
  request: HeaderMap,
) -> Result<axum::response::Response, ApiError> {
  use axum::{body::Body, response::IntoResponse};
  use tokio::io::{AsyncReadExt, AsyncSeekExt};
  let mut opened = attachments::open(&app, &id, &blob).await?;
  let length = opened.length;
  let etag = format!("\"{blob}\"");
  let mut headers = HeaderMap::new();
  headers.insert(header::CONTENT_TYPE, opened.mime_type.parse().unwrap());
  headers.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
  headers.insert(header::ETAG, etag.parse().unwrap());
  headers
    .insert(header::HeaderName::from_static("x-content-type-options"), "nosniff".parse().unwrap());
  let range = request
    .get(header::RANGE)
    .filter(|_| request.get(header::IF_RANGE).is_none_or(|v| v == etag.as_str()));
  let (start, end, status) = if let Some(value) = range {
    let parsed = value.to_str().ok().and_then(|s| byte_range(s, length));
    let Some((start, end)) = parsed else {
      headers.insert(header::CONTENT_RANGE, format!("bytes */{length}").parse().unwrap());
      return Ok((StatusCode::RANGE_NOT_SATISFIABLE, headers, Body::empty()).into_response());
    };
    headers.insert(header::CONTENT_RANGE, format!("bytes {start}-{end}/{length}").parse().unwrap());
    (start, end + 1, StatusCode::PARTIAL_CONTENT)
  } else {
    (0, length, StatusCode::OK)
  };
  headers.insert(header::CONTENT_LENGTH, (end - start).to_string().parse().unwrap());
  opened
    .file
    .seek(std::io::SeekFrom::Start(start))
    .await
    .map_err(crate::application::error::ApplicationError::internal)?;
  let stream = futures_util::stream::try_unfold(
    (opened.file, end - start),
    |(mut file, remaining)| async move {
      if remaining == 0 {
        return Ok::<_, std::io::Error>(None);
      }
      let mut chunk = vec![0u8; remaining.min(64 * 1024) as usize];
      let read = file.read(&mut chunk).await?;
      if read == 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "blob truncated"));
      }
      chunk.truncate(read);
      Ok(Some((chunk, (file, remaining - read as u64))))
    },
  );
  Ok((status, headers, Body::from_stream(stream)).into_response())
}
fn byte_range(value: &str, length: u64) -> Option<(u64, u64)> {
  if length == 0 {
    return None;
  }
  let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
  if start.is_empty() {
    let suffix: u64 = end.parse().ok()?;
    return (suffix > 0).then(|| (length.saturating_sub(suffix), length - 1));
  }
  let start: u64 = start.parse().ok()?;
  let end = if end.is_empty() { length - 1 } else { end.parse::<u64>().ok()?.min(length - 1) };
  (start < length && start <= end).then_some((start, end))
}
pub async fn metadata(
  State(app): State<Arc<App>>,
  Path((id, blob)): Path<(String, String)>,
) -> Result<Json<BlobMetadata>, ApiError> {
  Ok(Json(attachments::metadata(&app, &id, &blob).await?))
}
pub async fn input(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Input>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  Ok((StatusCode::ACCEPTED, Json(session_commands::submit_input(app, id, input).await?)))
}

pub async fn upload_draft(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  body: Bytes,
) -> Result<Json<Blob>, ApiError> {
  Ok(Json(attachments::upload_draft(&app, &id, &body).await?))
}
