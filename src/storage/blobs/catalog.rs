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
  pub linked: bool,
  pub available: bool,
  pub name: Option<String>,
  pub local_path: Option<String>,
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
    let filename = entry.file_name().to_string_lossy().into_owned();
    let mut linked = None;
    let id = if let Some(id) = filename.strip_suffix(".json").filter(|id| is_blob_id(id)) {
      if directory.join(id).exists() {
        continue;
      }
      let mut bytes = Vec::new();
      std::fs::File::open(entry.path())
        .map_err(RepositoryError::internal)?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(RepositoryError::internal)?;
      if bytes.len() > 65536 {
        continue;
      }
      let Ok(blob) = serde_json::from_slice::<super::Blob>(&bytes) else {
        continue;
      };
      if !blob.linked || blob.id != id {
        continue;
      }
      linked = Some(blob);
      id.to_owned()
    } else {
      filename
    };
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
    files.push((modified_at, id, linked));
  }
  files.sort_unstable_by(|a, b| (&b.0, &b.1).cmp(&(&a.0, &a.1)));
  let total = files.len();
  let mut items = Vec::new();
  for (modified_at, id, linked) in files.into_iter().skip(start).take(limit) {
    let path = linked
      .as_ref()
      .map(|blob| std::path::PathBuf::from(&blob.path))
      .unwrap_or_else(|| directory.join(&id));
    let name =
      linked.as_ref().and_then(|_| path.file_name()).map(|n| n.to_string_lossy().into_owned());
    let local_path = linked.as_ref().map(|blob| blob.path.clone());
    if let Some(blob) = &linked
      && !std::fs::metadata(&path).is_ok_and(|m| m.is_file())
    {
      items.push(Item {
        id,
        mime_type: blob.mime_type.clone(),
        byte_count: blob.byte_count as u64,
        modified_at,
        linked: true,
        available: false,
        name,
        local_path,
      });
      continue;
    }
    let mut file = match std::fs::File::open(path) {
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
    items.push(Item {
      id,
      mime_type,
      byte_count,
      modified_at,
      linked: linked.is_some(),
      available: true,
      name,
      local_path,
    });
  }
  let end = start.saturating_add(limit);
  Ok(Page { next: (end < total).then_some(end), items, total })
}
