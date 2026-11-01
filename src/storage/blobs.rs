//! A session's blob store, one directory per session (see `DataDir::blobs`): uploaded attachments,
//! each with a JSON description beside it, and the images its conversation holds. A blob is named
//! by the SHA-256 of its bytes, so the same bytes are stored once.
use crate::storage::repository_error::RepositoryError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
pub mod catalog;
pub mod linked;
pub const MAX_UPLOAD_BYTES: usize = 1024 * 1024 * 1024;

/// An upload's description, saved beside it.
#[derive(Clone, Serialize, Deserialize)]
pub struct Blob {
  pub id: String,
  pub mime_type: String,
  pub byte_count: usize,
  /// Where the blob is, canonical, for a shell command to read.
  pub path: String,
  #[serde(default, skip_serializing_if = "std::ops::Not::not")]
  pub linked: bool,
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
  save_upload_stream(
    directory,
    futures_util::stream::once(async { Ok::<_, std::io::Error>(body) }),
    MAX_UPLOAD_BYTES,
  )
  .await
}

/// Hash and stage an upload incrementally. Cancellation drops the unpublished temporary file.
pub async fn save_upload_stream<S, B, E>(
  directory: &Path,
  stream: S,
  limit: usize,
) -> Result<Blob, RepositoryError>
where
  S: futures_util::Stream<Item = Result<B, E>>,
  B: AsRef<[u8]>,
  E: std::fmt::Display,
{
  use futures_util::StreamExt;
  use tokio::io::AsyncWriteExt;
  tokio::fs::create_dir_all(directory).await.map_err(RepositoryError::internal)?;
  struct Remove(PathBuf);
  impl Drop for Remove {
    fn drop(&mut self) {
      let _ = std::fs::remove_file(&self.0);
    }
  }
  let temporary = Remove(directory.join(format!(".{}.tmp", uuid::Uuid::new_v4())));
  let mut file = tokio::fs::File::create(&temporary.0).await.map_err(RepositoryError::internal)?;
  let mut hash = Sha256::new();
  let mut count = 0usize;
  let mut prefix = Vec::new();
  futures_util::pin_mut!(stream);
  while let Some(chunk) = stream.next().await {
    let chunk = chunk.map_err(RepositoryError::internal)?;
    let bytes = chunk.as_ref();
    count = count.saturating_add(bytes.len());
    if count > limit.min(MAX_UPLOAD_BYTES) {
      return Err(RepositoryError::invalid("file exceeds upload or pending draft size limit"));
    }
    prefix.extend_from_slice(&bytes[..bytes.len().min(512 - prefix.len())]);
    hash.update(bytes);
    file.write_all(bytes).await.map_err(RepositoryError::internal)?;
  }
  file.sync_all().await.map_err(RepositoryError::internal)?;
  drop(file);
  let id = format!("{:x}", hash.finalize());
  let path = directory.join(&id);
  tokio::fs::rename(&temporary.0, &path).await.map_err(RepositoryError::internal)?;
  let blob = Blob {
    id: id.clone(),
    mime_type: sniff_mime_type(&prefix).into(),
    byte_count: count,
    path: tokio::fs::canonicalize(&path)
      .await
      .map_err(RepositoryError::internal)?
      .to_string_lossy()
      .into_owned(),
    linked: false,
  };
  atomic_write(
    &description_path(directory, &id),
    &serde_json::to_vec(&blob).map_err(RepositoryError::internal)?,
  )
  .await
  .map_err(RepositoryError::internal)?;
  Ok(blob)
}

/// Detect media and UTF-8 text consistently for uploads, catalogs and downloads.
pub(crate) fn sniff_mime_type(bytes: &[u8]) -> &'static str {
  crate::utils::image_format::supported_mime(bytes)
    .or_else(|| super::media::mime(bytes))
    .unwrap_or_else(|| {
      let text = std::str::from_utf8(bytes).or_else(|error| {
        if error.error_len().is_none() {
          std::str::from_utf8(&bytes[..error.valid_up_to()])
        } else {
          Err(error)
        }
      });
      if text
        .is_ok_and(|text| text.chars().all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t')))
      {
        "text/plain"
      } else {
        "application/octet-stream"
      }
    })
}

/// An upload's description; one without a readable description is not found.
pub async fn read_description(directory: &Path, id: &str) -> Result<Blob, RepositoryError> {
  let bytes = crate::utils::bounded_file::read(&description_path(directory, id), 64 * 1024)
    .await
    .map_err(|_| RepositoryError::not_found())?;
  let blob: Blob = serde_json::from_slice(&bytes).map_err(RepositoryError::internal)?;
  if blob.linked { linked::describe(&blob.id, &blob.path).await } else { Ok(blob) }
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
