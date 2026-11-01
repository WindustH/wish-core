//! Browsing the server's directories, for choosing a session's working directory.
use crate::application::error::{ApplicationError, blocking};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct DirectoryQuery {
  path: String,
  #[serde(default)]
  include_files: bool,
}
#[derive(Serialize)]
pub struct DirectoryListing {
  path: String,
  parent: Option<String>,
  directories: Vec<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  files: Option<Vec<String>>,
}

pub async fn list(query: DirectoryQuery) -> Result<DirectoryListing, ApplicationError> {
  blocking(move || {
    let path = if query.path == "~" {
      std::env::home_dir()
        .ok_or_else(|| ApplicationError::invalid("Server home directory is unavailable"))?
    } else {
      PathBuf::from(query.path)
    };
    if !path.is_absolute() {
      return Err(ApplicationError::invalid("Directory path must be absolute"));
    }
    let path = strip_verbatim(
      path
        .canonicalize()
        .map_err(|error| ApplicationError::invalid(format!("Cannot read directory: {error}")))?,
    );
    let entries = std::fs::read_dir(&path)
      .map_err(|error| ApplicationError::invalid(format!("Cannot read directory: {error}")))?;
    let mut directories = Vec::new();
    let mut files = query.include_files.then(Vec::new);
    for entry in entries {
      let entry = entry
        .map_err(|error| ApplicationError::invalid(format!("Cannot read directory: {error}")))?;
      let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
      let kind = entry
        .file_type()
        .map_err(|error| ApplicationError::invalid(format!("Cannot read directory: {error}")))?;
      // Follow links only to identify browsable directories / regular files.
      let metadata = if kind.is_symlink() { std::fs::metadata(entry.path()).ok() } else { None };
      if kind.is_dir() || metadata.as_ref().is_some_and(|value| value.is_dir()) {
        directories.push(name);
      } else if (kind.is_file() || metadata.as_ref().is_some_and(|value| value.is_file()))
        && let Some(files) = &mut files
      {
        files.push(name);
      }
    }
    directories.sort_unstable();
    if let Some(files) = &mut files {
      files.sort_unstable();
    }
    Ok(DirectoryListing {
      parent: path.parent().map(|p| p.to_string_lossy().into_owned()),
      path: path.to_string_lossy().into_owned(),
      directories,
      files,
    })
  })
  .await
}

/// Windows canonical paths come back as `\\?\C:\...` or `\\?\UNC\server\share\...`, a form
/// cmd.exe cannot use as its current directory. Such a path is rewritten as `C:\...` or
/// `\\server\share\...`; paths on other systems have no prefix and stay as they are.
fn strip_verbatim(path: PathBuf) -> PathBuf {
  use std::path::{Component, Prefix};
  let mut components = path.components();
  let root = match components.next() {
    Some(Component::Prefix(prefix)) => match prefix.kind() {
      Prefix::VerbatimDisk(letter) => format!("{}:\\", letter as char),
      Prefix::VerbatimUNC(server, share) => {
        format!("\\\\{}\\{}\\", server.to_string_lossy(), share.to_string_lossy())
      }
      _ => return path,
    },
    _ => return path,
  };
  let mut simplified = PathBuf::from(root);
  simplified.extend(components.filter(|component| !matches!(component, Component::RootDir)));
  simplified
}
