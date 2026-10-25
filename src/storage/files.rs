//! Filesystem accounting and deletion of explicitly selected stored resources.
use std::path::{Path, PathBuf};
/// A database file's companion, such as its `-wal` log.
pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
  let mut name = path.as_os_str().to_owned();
  name.push(suffix);
  name.into()
}

/// The bytes under a path: a file's length, or everything a directory holds without following
/// links. A path that does not exist holds none.
pub fn size(path: &Path) -> std::io::Result<u64> {
  if !path.exists() {
    return Ok(0);
  }
  if path.is_file() {
    return Ok(path.metadata()?.len());
  }
  let mut total = 0;
  for entry in std::fs::read_dir(path)? {
    let entry = entry?;
    if !entry.file_type()?.is_symlink() {
      total += size(&entry.path())?;
    }
  }
  Ok(total)
}

/// One persisted resource and its expected filesystem kind.
pub enum ResourceFiles {
  File { path: PathBuf, companions: Vec<PathBuf> },
  Directory { path: PathBuf },
}
#[derive(Default)]
pub struct ReleasedFiles {
  pub resources: u64,
  pub bytes: u64,
}
/// Removes selected resources, or measures them without writing when `apply` is false.
/// The caller has already excluded resources still in use.
pub fn release_resources(
  resources: &[ResourceFiles],
  apply: bool,
) -> std::io::Result<ReleasedFiles> {
  let mut released = ReleasedFiles::default();
  for resource in resources {
    match resource {
      ResourceFiles::File { path, companions } => {
        if !path.is_file() {
          continue;
        }
        released.resources += 1;
        for path in std::iter::once(path).chain(companions) {
          released.bytes += size(path)?;
          if apply {
            ignore_missing(std::fs::remove_file(path))?;
          }
        }
      }
      ResourceFiles::Directory { path } => {
        if !path.is_dir() {
          continue;
        }
        released.resources += 1;
        released.bytes += size(path)?;
        if apply {
          ignore_missing(std::fs::remove_dir_all(path))?;
        }
      }
    }
  }
  Ok(released)
}
fn ignore_missing(result: std::io::Result<()>) -> std::io::Result<()> {
  match result {
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
    result => result,
  }
}
/// The database file and its SQLite journal companions.
pub fn database_size(path: &Path) -> std::io::Result<u64> {
  Ok(size(path)? + size(&with_suffix(path, "-wal"))? + size(&with_suffix(path, "-shm"))?)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn preview_and_release_only_selected_resources() -> std::io::Result<()> {
    let root = std::env::temp_dir().join(format!("wish-resource-release-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("execution"))?;
    std::fs::write(root.join("blob"), b"image")?;
    std::fs::write(root.join("description"), b"metadata")?;
    std::fs::write(root.join("execution/stdout"), b"output")?;
    std::fs::write(root.join("retained"), b"still referenced")?;
    let resources = [
      ResourceFiles::File { path: root.join("blob"), companions: vec![root.join("description")] },
      ResourceFiles::Directory { path: root.join("execution") },
    ];
    let preview = release_resources(&resources, false)?;
    assert_eq!(preview.resources, 2);
    assert_eq!(preview.bytes, 19);
    assert_eq!(std::fs::read(root.join("blob"))?, b"image");
    assert_eq!(std::fs::read(root.join("description"))?, b"metadata");
    assert_eq!(std::fs::read(root.join("execution/stdout"))?, b"output");

    let released = release_resources(&resources, true)?;
    assert_eq!((released.resources, released.bytes), (preview.resources, preview.bytes));
    assert!(!root.join("blob").exists());
    assert!(!root.join("description").exists());
    assert!(!root.join("execution").exists());
    assert_eq!(std::fs::read(root.join("retained"))?, b"still referenced");
    let again = release_resources(&resources, true)?;
    assert_eq!((again.resources, again.bytes), (0, 0));
    std::fs::remove_dir_all(root)
  }
}
