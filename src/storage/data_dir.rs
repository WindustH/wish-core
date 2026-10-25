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
