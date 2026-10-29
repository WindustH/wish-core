//! Bounded streaming artifacts, published only after a complete validated download.
use super::{
  blobs::{Blob, description_path},
  repository_error::RepositoryError,
};
use futures_util::{Stream, StreamExt};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

struct Temporary(PathBuf);
impl Drop for Temporary {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.0);
  }
}

pub fn mime(bytes: &[u8]) -> Option<&'static str> {
  if bytes.starts_with(b"PK\x03\x04") {
    return Some("application/zip");
  }
  if bytes.starts_with(b"ID3") || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
  {
    return Some("audio/mpeg");
  }
  if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
    return Some("audio/wav");
  }
  if bytes.starts_with(b"fLaC") {
    return Some("audio/flac");
  }
  if bytes.starts_with(b"OggS") {
    return Some("audio/ogg");
  }
  if bytes.get(4..8) == Some(b"ftyp") {
    return Some(if bytes.get(8..12) == Some(b"qt  ") { "video/quicktime" } else { "video/mp4" });
  }
  if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
    return Some("video/webm");
  }
  None
}

pub type Validator = fn(&Path) -> Result<(), String>;
pub async fn save_validated<S, E>(
  directory: &Path,
  mut chunks: S,
  kind: &str,
  limit: usize,
  validator: Option<Validator>,
) -> Result<Blob, RepositoryError>
where
  S: Stream<Item = Result<Vec<u8>, E>> + Unpin,
  E: std::fmt::Display,
{
  tokio::fs::create_dir_all(directory).await.map_err(RepositoryError::internal)?;
  let temporary = Temporary(directory.join(format!(".{}.part", uuid::Uuid::new_v4())));
  let mut file = tokio::fs::File::create(&temporary.0).await.map_err(RepositoryError::internal)?;
  let mut hash = Sha256::new();
  let mut prefix = Vec::new();
  let mut count = 0usize;
  while let Some(chunk) = chunks.next().await {
    let chunk = chunk.map_err(RepositoryError::internal)?;
    count = count
      .checked_add(chunk.len())
      .filter(|n| *n <= limit)
      .ok_or_else(|| RepositoryError::invalid("generated media exceeds size limit"))?;
    if prefix.len() < 512 {
      prefix.extend_from_slice(&chunk[..chunk.len().min(512 - prefix.len())]);
    }
    hash.update(&chunk);
    file.write_all(&chunk).await.map_err(RepositoryError::internal)?;
  }
  let mime = mime(&prefix)
    .filter(|mime| mime.starts_with(kind))
    .ok_or_else(|| RepositoryError::invalid("generated artifact is not a supported media file"))?;
  file.sync_all().await.map_err(RepositoryError::internal)?;
  drop(file);
  if let Some(validator) = validator {
    let path = temporary.0.clone();
    crate::utils::blocking::cpu(move || validator(&path))
      .await
      .map_err(RepositoryError::internal)?
      .map_err(RepositoryError::invalid)?;
  }
  let id = format!("{:x}", hash.finalize());
  let path = directory.join(&id);
  tokio::fs::rename(&temporary.0, &path).await.map_err(RepositoryError::internal)?;
  let blob = Blob {
    id: id.clone(),
    mime_type: mime.into(),
    byte_count: count,
    path: tokio::fs::canonicalize(path)
      .await
      .map_err(RepositoryError::internal)?
      .to_string_lossy()
      .into_owned(),
  };
  let meta = Temporary(directory.join(format!(".{}.json.tmp", uuid::Uuid::new_v4())));
  tokio::fs::write(&meta.0, serde_json::to_vec(&blob).map_err(RepositoryError::internal)?)
    .await
    .map_err(RepositoryError::internal)?;
  tokio::fs::rename(&meta.0, description_path(directory, &id))
    .await
    .map_err(RepositoryError::internal)?;
  Ok(blob)
}
