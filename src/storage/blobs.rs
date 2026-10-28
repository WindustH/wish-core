//! A session's blob store, one directory per session (see `DataDir::blobs`): uploaded attachments,
//! each with a JSON description beside it, and the images its conversation holds. A blob is named
//! by the SHA-256 of its bytes, so the same bytes are stored once.
use crate::storage::repository_error::RepositoryError;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// An upload's description, saved beside it.
#[derive(Serialize, Deserialize)]
pub struct Blob {
  pub id: String,
  pub mime_type: String,
  pub byte_count: usize,
  /// Where the blob is, canonical, for a shell command to read.
  pub path: String,
}

/// Whether a name is a blob's: a SHA-256 in hex.
pub fn is_blob_id(id: &str) -> bool {
  id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A blob's id for its bytes.
pub fn blob_id(bytes: &[u8]) -> String {
  format!("{:x}", Sha256::digest(bytes))
}

/// Where an upload's description is.
pub fn description_path(directory: &Path, id: &str) -> PathBuf {
  directory.join(format!("{id}.json"))
}

/// Stores an upload and its description.
pub async fn save_upload(directory: &Path, body: &[u8]) -> Result<Blob, RepositoryError> {
  let id = blob_id(body);
  tokio::fs::create_dir_all(directory).await.map_err(RepositoryError::internal)?;
  let path = directory.join(&id);
  atomic_write(&path, body).await.map_err(RepositoryError::internal)?;
  let blob = Blob {
    id: id.clone(),
    mime_type: sniff_mime_type(body).into(),
    byte_count: body.len(),
    path: tokio::fs::canonicalize(&path)
      .await
      .map_err(RepositoryError::internal)?
      .to_string_lossy()
      .into_owned(),
  };
  atomic_write(
    &description_path(directory, &id),
    &serde_json::to_vec(&blob).map_err(RepositoryError::internal)?,
  )
  .await
  .map_err(RepositoryError::internal)?;
  Ok(blob)
}

/// The image formats a model reads, by their magic bytes; anything else is octet-stream.
fn sniff_mime_type(bytes: &[u8]) -> &'static str {
  crate::utils::image_format::supported_mime(bytes)
    .or_else(|| super::media::mime(bytes))
    .unwrap_or("application/octet-stream")
}

/// An upload's description; one without a readable description is not found.
pub async fn read_description(directory: &Path, id: &str) -> Result<Blob, RepositoryError> {
  let bytes = tokio::fs::read(description_path(directory, id))
    .await
    .map_err(|_| RepositoryError::not_found())?;
  serde_json::from_slice(&bytes).map_err(RepositoryError::internal)
}

/// Stores an image the conversation holds as base64, and returns where it is.
pub fn save_image(directory: &Path, data: &str) -> Result<PathBuf, RepositoryError> {
  let bytes = base64::engine::general_purpose::STANDARD
    .decode(data)
    .map_err(|e| RepositoryError::Invalid(format!("invalid image: {e}")))?;
  save_image_bytes(directory, &bytes)
}

pub fn save_image_bytes(directory: &Path, bytes: &[u8]) -> Result<PathBuf, RepositoryError> {
  let path = directory.join(blob_id(bytes));
  // Content-addressed blobs are immutable; avoid rewriting every image on every model call.
  if std::fs::metadata(&path).is_ok_and(|meta| meta.len() == bytes.len() as u64) {
    return Ok(path);
  }
  let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
  std::fs::create_dir_all(directory)
    .and_then(|_| std::fs::write(&temporary, bytes))
    .and_then(|_| std::fs::rename(&temporary, &path))
    .map_err(|error| {
      let _ = std::fs::remove_file(&temporary);
      RepositoryError::Invalid(format!("save session image: {error}"))
    })?;
  Ok(path)
}

/// Publish complete bytes without truncating an existing content-addressed file.
pub async fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
  use tokio::io::AsyncWriteExt;
  let temporary = path.with_file_name(format!(".{}.tmp", uuid::Uuid::new_v4()));
  // Guard also removes abandoned files on ordinary cancellation.
  struct Remove(std::path::PathBuf);
  impl Drop for Remove {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }
  let guard = Remove(temporary);
  let mut file = tokio::fs::File::create(&guard.0).await?;
  file.write_all(bytes).await?;
  file.sync_all().await?;
  drop(file);
  tokio::fs::rename(&guard.0, path).await
}
