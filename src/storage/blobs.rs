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
  tokio::fs::write(&path, body).await.map_err(RepositoryError::internal)?;
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
  tokio::fs::write(description_path(directory, &id), serde_json::to_vec(&blob).unwrap())
    .await
    .map_err(RepositoryError::internal)?;
  Ok(blob)
}

/// The image formats a model reads, by their magic bytes; anything else is octet-stream.
fn sniff_mime_type(bytes: &[u8]) -> &'static str {
  if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
    "image/png"
  } else if bytes.starts_with(b"\xff\xd8\xff") {
    "image/jpeg"
  } else if bytes.starts_with(b"GIF8") {
    "image/gif"
  } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
    "image/webp"
  } else {
    "application/octet-stream"
  }
}

/// A blob's bytes.
pub async fn read(directory: &Path, id: &str) -> Result<Vec<u8>, RepositoryError> {
  tokio::fs::read(directory.join(id)).await.map_err(|error| {
    if error.kind() == std::io::ErrorKind::NotFound {
      RepositoryError::not_found()
    } else {
      RepositoryError::internal(error)
    }
  })
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
  let path = directory.join(blob_id(&bytes));
  let temporary = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
  std::fs::create_dir_all(directory)
    .and_then(|_| std::fs::write(&temporary, &bytes))
    .and_then(|_| std::fs::rename(&temporary, &path))
    .map_err(|error| {
      let _ = std::fs::remove_file(&temporary);
      RepositoryError::Invalid(format!("save session image: {error}"))
    })?;
  Ok(path)
}
