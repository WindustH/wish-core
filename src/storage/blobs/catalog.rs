//! Paged blob descriptions, including explicit local links; file contents are never returned.
use super::is_blob_id;
use crate::storage::repository_error::RepositoryError;
use serde::Serialize;
use std::{
  collections::VecDeque,
  io::Read,
  path::{Path, PathBuf},
  sync::{Arc, LazyLock, Mutex},
  time::{SystemTime, UNIX_EPOCH},
};
#[derive(Clone)]
struct Row {
  modified_at: u64,
  id: String,
  linked_blob: Option<super::Blob>,
  presentation: super::library::Entry,
}
// Blob publication and link registration use atomic renames. Directory revision changes
// invalidate this bounded index; linked source contents are still inspected on each page.
struct Index {
  directory: PathBuf,
  revision: SystemTime,
  files: Arc<Vec<Row>>,
}
static INDEX: LazyLock<Mutex<VecDeque<Index>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));
fn revision(directory: &Path) -> Option<SystemTime> {
  std::fs::metadata(directory).ok()?.modified().ok()
}
fn indexed(directory: &Path) -> Result<Arc<Vec<Row>>, RepositoryError> {
  let before = revision(directory);
  if let Some(revision) = before {
    let mut cache =
      INDEX.lock().map_err(|_| RepositoryError::internal("catalog index lock poisoned"))?;
    if let Some(position) =
      cache.iter().position(|item| item.directory == directory && item.revision == revision)
    {
      let item = cache.remove(position).expect("located index");
      let files = item.files.clone();
      cache.push_back(item);
      return Ok(files);
    }
  }
  let files = Arc::new(scan(directory)?);
  if let Some(before) = before
    && Some(before) == revision(directory)
    && files.len() <= 8192
  {
    let mut cache =
      INDEX.lock().map_err(|_| RepositoryError::internal("catalog index lock poisoned"))?;
    cache.retain(|item| item.directory != directory);
    cache.push_back(Index { directory: directory.into(), revision: before, files: files.clone() });
    while cache.len() > 64 || cache.iter().map(|item| item.files.len()).sum::<usize>() > 8192 {
      cache.pop_front();
    }
  }
  Ok(files)
}

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
  pub visible: bool,
  pub pinned: bool,
}
#[derive(Serialize)]
pub struct Page {
  pub items: Vec<Item>,
  pub next: Option<usize>,
  pub total: usize,
}

fn scan(directory: &Path) -> Result<Vec<Row>, RepositoryError> {
  let library = super::library::entries(directory)?;
  let entries = match std::fs::read_dir(directory) {
    Ok(entries) => entries,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
      return Ok(vec![]);
    }
    Err(error) => return Err(RepositoryError::internal(error)),
  };
  let mut files = Vec::new();
  for entry in entries {
    let entry = entry.map_err(RepositoryError::internal)?;
    let filename = entry.file_name().to_string_lossy().into_owned();
    let mut linked_blob = None;
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
      linked_blob = Some(blob);
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
    let presentation = library.get(&id).cloned().unwrap_or_default();
    files.push(Row { modified_at, id, linked_blob, presentation });
  }
  files.sort_unstable_by(|a, b| {
    (&b.presentation.pinned, &b.modified_at, &b.id).cmp(&(
      &a.presentation.pinned,
      &a.modified_at,
      &a.id,
    ))
  });
  Ok(files)
}
pub fn contains(directory: &Path, id: &str) -> Result<bool, RepositoryError> {
  if !is_blob_id(id) {
    return Ok(false);
  }
  match std::fs::symlink_metadata(directory.join(id)) {
    Ok(metadata) => return Ok(metadata.is_file()),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
    Err(error) => return Err(RepositoryError::internal(error)),
  }
  let path = super::description_path(directory, id);
  match std::fs::symlink_metadata(&path) {
    Ok(metadata) if metadata.is_file() && metadata.len() <= 64 * 1024 => {}
    Ok(_) => return Ok(false),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
    Err(error) => return Err(RepositoryError::internal(error)),
  }
  let bytes =
    crate::utils::bounded_file::read_sync(&path, 64 * 1024).map_err(RepositoryError::internal)?;
  Ok(serde_json::from_slice::<super::Blob>(&bytes).is_ok_and(|blob| blob.linked && blob.id == id))
}
pub fn list(
  directory: &Path,
  start: usize,
  limit: usize,
  include_hidden: bool,
) -> Result<Page, RepositoryError> {
  let files = indexed(directory)?;
  let files: Vec<_> =
    files.iter().filter(|row| include_hidden || row.presentation.visible).collect();
  let total = files.len();
  let mut items = Vec::new();
  for Row { modified_at, id, linked_blob, presentation } in
    files.into_iter().skip(start).take(limit).cloned()
  {
    let path = linked_blob
      .as_ref()
      .map(|blob| std::path::PathBuf::from(&blob.path))
      .unwrap_or_else(|| directory.join(&id));
    let name = presentation.name.clone().or_else(|| {
      linked_blob.as_ref().and_then(|_| path.file_name()).map(|n| n.to_string_lossy().into_owned())
    });
    let local_path = linked_blob.as_ref().map(|blob| blob.path.clone());
    if let Some(blob) = &linked_blob
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
        visible: presentation.visible,
        pinned: presentation.pinned,
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
    let mime_type = super::sniff_mime_type(prefix).to_owned();
    // Model images can exist without an upload sidecar; inspect only a small prefix.
    items.push(Item {
      id,
      mime_type,
      byte_count,
      modified_at,
      linked: linked_blob.is_some(),
      available: true,
      name,
      local_path,
      visible: presentation.visible,
      pinned: presentation.pinned,
    });
  }
  let end = start.saturating_add(limit);
  Ok(Page { next: (end < total).then_some(end), items, total })
}
