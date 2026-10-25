//! Uploads and reads attachments owned by a session or group.
use super::{app::App, error::ApplicationError};
use crate::storage::blobs;
pub use crate::storage::blobs::Blob;
use serde::Serialize;
use std::sync::Arc;
/// Refuses an id that names neither a session nor a group: the owners of `blobs/<id>`.
async fn require_owner(app: &Arc<App>, id: &str) -> Result<(), ApplicationError> {
  if app.management.group(id)?.is_none() {
    app.get_session(id).await?;
  }
  Ok(())
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
pub async fn download(
  app: &Arc<App>,
  owner: &str,
  blob: &str,
) -> Result<Vec<u8>, ApplicationError> {
  require_owner(app, owner).await?;
  if !blobs::is_blob_id(blob) {
    return Err(ApplicationError::not_found());
  }
  Ok(blobs::read(&app.data_dir.blobs(owner), blob).await?)
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
