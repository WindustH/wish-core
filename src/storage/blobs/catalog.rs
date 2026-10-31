//! Paged descriptions of published blobs. No file contents or local paths leave this catalog.
use super::is_blob_id;
use crate::storage::repository_error::RepositoryError;
use serde::Serialize;
use std::{io::Read, path::Path, time::UNIX_EPOCH};

#[derive(Serialize)]
pub struct Item {
  pub id: String,
  pub mime_type: String,
  pub byte_count: u64,
  pub modified_at: u64,
}
#[derive(Serialize)]
pub struct Page {
  pub items: Vec<Item>,
  pub next: Option<usize>,
  pub total: usize,
}

pub fn list(directory: &Path, start: usize, limit: usize) -> Result<Page, RepositoryError> {
  let entries = match std::fs::read_dir(directory) {
    Ok(entries) => entries,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
      return Ok(Page { items: vec![], next: None, total: 0 });
    }
    Err(error) => return Err(RepositoryError::internal(error)),
  };
  let mut files = Vec::new();
  for entry in entries {
    let entry = entry.map_err(RepositoryError::internal)?;
    let id = entry.file_name().to_string_lossy().into_owned();
    if !is_blob_id(&id) || !entry.file_type().map_err(RepositoryError::internal)?.is_file() {
      continue;
    }
    let metadata = match entry.metadata() {
      Ok(metadata) => metadata,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
      Err(error) => return Err(RepositoryError::internal(error)),
    };
    let modified_at = metadata
      .modified()
      .ok()
      .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
      .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
      .unwrap_or(0);
    files.push((modified_at, id));
  }
  files.sort_unstable_by(|a, b| b.cmp(a));
  let total = files.len();
  let mut items = Vec::new();
  for (modified_at, id) in files.into_iter().skip(start).take(limit) {
    let mut file = match std::fs::File::open(directory.join(&id)) {
      Ok(file) => file,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
      Err(error) => return Err(RepositoryError::internal(error)),
    };
    let byte_count = file.metadata().map_err(RepositoryError::internal)?.len();
    let mut prefix = [0; 512];
    let length = file.read(&mut prefix).map_err(RepositoryError::internal)?;
    let prefix = &prefix[..length];
    let detected = super::sniff_mime_type(prefix);
    let text = std::str::from_utf8(prefix).or_else(|error| {
      if error.error_len().is_none() {
        std::str::from_utf8(&prefix[..error.valid_up_to()])
      } else {
        Err(error)
      }
    });
    let mime_type = if detected == "application/octet-stream"
      && text
        .is_ok_and(|text| text.chars().all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t')))
    {
      "text/plain"
    } else {
      detected
    }
    .to_owned();
    // Model images can exist without an upload sidecar; inspect only a small prefix.
    items.push(Item { id, mime_type, byte_count, modified_at });
  }
  let end = start.saturating_add(limit);
  Ok(Page { next: (end < total).then_some(end), items, total })
}
