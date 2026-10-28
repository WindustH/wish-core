//! Uploads and reads attachments owned by a session or group.
use super::{app::App, error::ApplicationError};
use crate::storage::blobs;
pub use crate::storage::blobs::Blob;
use serde::Serialize;
use std::sync::Arc;
/// Refuses an id that names neither a session nor a group: the owners of `blobs/<id>`.
async fn require_owner(app: &Arc<App>, id: &str) -> Result<(), ApplicationError> {
  let management = app.management.clone();
  let id = id.to_owned();
  super::error::blocking(move || {
    if management.group(&id)?.is_none() && !management.exists(&id)? {
      return Err(ApplicationError::not_found());
    }
    Ok(())
  })
  .await
}
#[derive(Serialize)]
pub struct BlobMetadata {
  id: String,
  mime_type: String,
  byte_count: usize,
}

pub async fn upload(app: &Arc<App>, owner: &str, body: &[u8]) -> Result<Blob, ApplicationError> {
  app.lifecycle.require_open()?;
  require_owner(app, owner).await?;
  Ok(blobs::save_upload(&app.data_dir.blobs(owner), body).await?)
}
pub struct BlobReader {
  pub file: tokio::fs::File,
  pub length: u64,
  pub mime_type: String,
}
pub async fn open(app: &Arc<App>, owner: &str, blob: &str) -> Result<BlobReader, ApplicationError> {
  use tokio::io::{AsyncReadExt, AsyncSeekExt};
  require_owner(app, owner).await?;
  if !blobs::is_blob_id(blob) {
    return Err(ApplicationError::not_found());
  }
  let mut file =
    tokio::fs::File::open(app.data_dir.blobs(owner).join(blob)).await.map_err(|error| {
      if error.kind() == std::io::ErrorKind::NotFound {
        ApplicationError::not_found()
      } else {
        ApplicationError::internal(error)
      }
    })?;
  let length = file.metadata().await.map_err(ApplicationError::internal)?.len();
  let mut prefix = [0; 512];
  let count = file.read(&mut prefix).await.map_err(ApplicationError::internal)?;
  file.rewind().await.map_err(ApplicationError::internal)?;
  let mime_type = crate::utils::image_format::supported_mime(&prefix[..count])
    .or_else(|| crate::storage::media::mime(&prefix[..count]))
    .unwrap_or("application/octet-stream")
    .into();
  Ok(BlobReader { file, length, mime_type })
}
pub async fn metadata(
  app: &Arc<App>,
  owner: &str,
  blob: &str,
) -> Result<BlobMetadata, ApplicationError> {
  require_owner(app, owner).await?;
  if !blobs::is_blob_id(blob) {
    return Err(ApplicationError::not_found());
  }
  let item = blobs::read_description(&app.data_dir.blobs(owner), blob).await?;
  Ok(BlobMetadata { id: item.id, mime_type: item.mime_type, byte_count: item.byte_count })
}

/// Stage bytes under a submission UUID without creating or publishing an empty session.
/// Content-addressed names make retries reuse the same file; acceptance adopts this directory.
pub async fn upload_draft(
  app: &Arc<App>,
  request_id: &str,
  body: &[u8],
) -> Result<Blob, ApplicationError> {
  app.lifecycle.require_open()?;
  let id = uuid::Uuid::parse_str(request_id)
    .map_err(|_| ApplicationError::invalid("draft request_id must be a UUID"))?
    .to_string();
  Ok(blobs::save_upload(&app.data_dir.blobs(&id), body).await?)
}
