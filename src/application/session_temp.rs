//! Stable scratch paths owned by a session, independent of its project's working directory.
use super::error::ApplicationError;
use std::path::{Path, PathBuf};

pub(super) fn allocate(id: &str) -> Result<PathBuf, ApplicationError> {
  std::path::absolute(std::env::temp_dir().join("wish-core").join(id))
    .map_err(ApplicationError::internal)
}

pub(super) async fn ensure(path: &Path) -> Result<(), ApplicationError> {
  let mut builder = tokio::fs::DirBuilder::new();
  builder.recursive(true);
  #[cfg(unix)]
  builder.mode(0o700);
  builder.create(path).await.map_err(ApplicationError::internal)?;
  if !tokio::fs::symlink_metadata(path).await.map_err(ApplicationError::internal)?.is_dir() {
    return Err(ApplicationError::invalid(
      "session temporary path must be a directory, not a link",
    ));
  }
  Ok(())
}

/// Only remove the scratch namespace allocated for this owner, never its project directory.
pub(super) fn validate(id: &str, path: &Path) -> Result<(), ApplicationError> {
  if !path.is_absolute()
    || path.file_name().and_then(|v| v.to_str()) != Some(id)
    || path.parent().and_then(Path::file_name).and_then(|v| v.to_str()) != Some("wish-core")
    || path.components().any(|part| matches!(part, std::path::Component::ParentDir))
  {
    return Err(ApplicationError::invalid(
      "refusing to remove a path outside the session scratch namespace",
    ));
  }
  Ok(())
}
pub(super) async fn remove(id: &str, path: Option<&Path>) -> Result<(), ApplicationError> {
  let Some(path) = path else { return Ok(()) };
  validate(id, path)?;
  match tokio::fs::remove_dir_all(path).await {
    Ok(()) => Ok(()),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
    Err(error) => Err(ApplicationError::internal(error)),
  }
}
