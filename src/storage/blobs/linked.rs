//! Explicit references to local files. Only a descriptor is stored; the source is never copied.
use super::*;
use tokio::io::AsyncReadExt;

pub async fn describe(id: &str, path: &str) -> Result<Blob, RepositoryError> {
  let metadata = tokio::fs::metadata(path).await.map_err(|_| RepositoryError::not_found())?;
  if !metadata.is_file() {
    return Err(RepositoryError::invalid("linked path must be a regular file"));
  }
  let mut file = tokio::fs::File::open(path).await.map_err(RepositoryError::internal)?;
  let mut prefix = [0; 512];
  let length = file.read(&mut prefix).await.map_err(RepositoryError::internal)?;
  Ok(Blob {
    id: id.into(),
    path: path.into(),
    linked: true,
    mime_type: sniff_mime_type(&prefix[..length]).into(),
    byte_count: usize::try_from(metadata.len()).map_err(RepositoryError::internal)?,
  })
}

pub async fn register(directory: &Path, path: &str) -> Result<Blob, RepositoryError> {
  if !Path::new(path).is_absolute() {
    return Err(RepositoryError::invalid("local file path must be absolute"));
  }
  let path = tokio::fs::canonicalize(path).await.map_err(|_| RepositoryError::not_found())?;
  let path =
    path.to_str().ok_or_else(|| RepositoryError::invalid("file path must be valid UTF-8"))?;
  let id = blob_id(format!("wish-local-file\0{path}").as_bytes());
  let blob = describe(&id, path).await?;
  tokio::fs::create_dir_all(directory).await.map_err(RepositoryError::internal)?;
  atomic_write(
    &description_path(directory, &id),
    &serde_json::to_vec(&blob).map_err(RepositoryError::internal)?,
  )
  .await
  .map_err(RepositoryError::internal)?;
  Ok(blob)
}

/// Ordinary blob paths retain their directory boundary; registered references opt in to external paths.
pub async fn resolve(directory: &Path, id: &str) -> Result<PathBuf, RepositoryError> {
  if !is_blob_id(id) {
    return Err(RepositoryError::not_found());
  }
  let path = directory.join(id);
  if tokio::fs::try_exists(&path).await.map_err(RepositoryError::internal)? {
    let root = tokio::fs::canonicalize(directory).await.map_err(RepositoryError::internal)?;
    let canonical = tokio::fs::canonicalize(path).await.map_err(RepositoryError::internal)?;
    if canonical.parent() != Some(root.as_path()) {
      return Err(RepositoryError::invalid("blob escapes its directory"));
    }
    return Ok(canonical);
  }
  let blob = read_description(directory, id).await?;
  if !blob.linked {
    return Err(RepositoryError::not_found());
  }
  Ok(PathBuf::from(blob.path))
}
