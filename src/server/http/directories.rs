use crate::server::error::{ApiError, blocking};
use axum::{Json, extract::Query};
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

pub async fn list(Query(query): Query<DirectoryQuery>) -> Result<Json<DirectoryListing>, ApiError> {
  blocking(move || {
    let path = if query.path == "~" {
      PathBuf::from(
        std::env::var_os("HOME")
          .ok_or_else(|| ApiError::bad_request("Server home directory is unavailable"))?,
      )
    } else {
      PathBuf::from(query.path)
    };
    if !path.is_absolute() {
      return Err(ApiError::bad_request("Directory path must be absolute"));
    }
    let path = path
      .canonicalize()
      .map_err(|error| ApiError::bad_request(format!("Cannot read directory: {error}")))?;
    let entries = std::fs::read_dir(&path)
      .map_err(|error| ApiError::bad_request(format!("Cannot read directory: {error}")))?;
    let mut directories = Vec::new();
    for entry in entries {
      let entry =
        entry.map_err(|error| ApiError::bad_request(format!("Cannot read directory: {error}")))?;
      if entry.path().is_dir() {
        if let Some(name) = entry.file_name().to_str() {
          directories.push(name.to_owned());
        }
      }
    }
    directories.sort_unstable();
    Ok(Json(DirectoryListing {
      parent: path.parent().map(|p| p.to_string_lossy().into_owned()),
      path: path.to_string_lossy().into_owned(),
      directories,
    }))
  })
  .await
}
