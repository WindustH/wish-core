//! Publish complete files without exposing partial writes; remove abandoned temporaries.
use std::path::{Path, PathBuf};

struct Temporary(PathBuf);
impl Drop for Temporary {
  fn drop(&mut self) {
    let _ = std::fs::remove_file(&self.0);
  }
}
pub fn write_sync(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
  use std::io::Write;
  let temporary = Temporary(path.with_file_name(format!(".{}.tmp", uuid::Uuid::new_v4())));
  let mut file = std::fs::File::create(&temporary.0)?;
  file.write_all(bytes)?;
  file.sync_all()?;
  drop(file);
  std::fs::rename(&temporary.0, path)
}
pub async fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
  write_with_permissions(path, bytes, None).await
}
pub async fn write_with_permissions(
  path: &Path,
  bytes: &[u8],
  permissions: Option<std::fs::Permissions>,
) -> std::io::Result<()> {
  use tokio::io::AsyncWriteExt;
  let temporary = Temporary(path.with_file_name(format!(".{}.tmp", uuid::Uuid::new_v4())));
  let mut file = tokio::fs::File::create(&temporary.0).await?;
  if let Some(permissions) = permissions {
    file.set_permissions(permissions).await?;
  }
  file.write_all(bytes).await?;
  file.sync_all().await?;
  drop(file);
  tokio::fs::rename(&temporary.0, path).await
}
