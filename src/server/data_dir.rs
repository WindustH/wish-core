//! The layout of the data directory, in one place: the two databases, and each session's files in
//! directories named by its id.
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct DataDir(PathBuf);

impl DataDir {
  pub fn new(root: PathBuf) -> Self {
    Self(root)
  }
  pub fn root(&self) -> &Path {
    &self.0
  }
  /// The session engine's database.
  pub fn database(&self) -> PathBuf {
    self.0.join("wish.sqlite")
  }
  /// The management index: session records, model calls and stream samples.
  pub fn management_database(&self) -> PathBuf {
    self.0.join("management.sqlite")
  }
  /// Every session's blobs.
  pub fn blob_root(&self) -> PathBuf {
    self.0.join("blobs")
  }
  /// A session's blobs: uploaded attachments and the images its conversation holds.
  pub fn blobs(&self, session: &str) -> PathBuf {
    self.blob_root().join(session)
  }
  /// Every session's shell output.
  pub fn shell_root(&self) -> PathBuf {
    self.0.join("shell")
  }
  /// A session's shell output, one directory per command.
  pub fn shell(&self, session: &str) -> PathBuf {
    self.shell_root().join(session)
  }
  /// Files a session's MCP calls returned, saved in place of their base64.
  pub fn mcp_files(&self, session: &str) -> PathBuf {
    self.shell(session).join("mcp")
  }
  /// Where a link to this program is put for sessions' shells (see `mcp::bridge`).
  pub fn bin(&self) -> PathBuf {
    self.0.join("bin")
  }
}

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
