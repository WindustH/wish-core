//! Browsing the server's directories, for choosing a session's working directory.
use crate::application::error::{ApplicationError, blocking};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Deserialize)]
pub struct DirectoryQuery {
  path: String,
}
#[derive(Serialize)]
pub struct DirectoryListing {
  path: String,
  parent: Option<String>,
  directories: Vec<String>,
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
    for entry in entries {
      let entry = entry
        .map_err(|error| ApplicationError::invalid(format!("Cannot read directory: {error}")))?;
      if entry.path().is_dir()
        && let Some(name) = entry.file_name().to_str()
      {
        directories.push(name.to_owned());
      }
    }
    directories.sort_unstable();
    Ok(DirectoryListing {
      parent: path.parent().map(|p| p.to_string_lossy().into_owned()),
      path: path.to_string_lossy().into_owned(),
      directories,
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
