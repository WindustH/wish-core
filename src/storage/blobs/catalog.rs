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
}
// Explicit invalidation complements directory mtime on filesystems with coarse timestamps.
struct Index {
  directory: PathBuf,
  revision: SystemTime,
  files: Arc<Vec<Row>>,
}
#[derive(Default)]
struct Cache {
  epoch: u64,
  entries: VecDeque<Index>,
}
static INDEX: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));

/// Invalidate even after a partially completed mutation. The epoch prevents an older
/// concurrent scan from republishing stale rows after this guard has been dropped.
pub(crate) struct Mutation<'a>(pub &'a Path);
impl Drop for Mutation<'_> {
  fn drop(&mut self) {
    let mut cache = INDEX.lock().unwrap_or_else(|error| error.into_inner());
    cache.epoch = cache.epoch.wrapping_add(1);
    cache.entries.retain(|item| item.directory != self.0);
  }
}
fn revision(directory: &Path) -> Option<SystemTime> {
  std::fs::metadata(directory).ok()?.modified().ok()
}
fn indexed(directory: &Path) -> Result<Arc<Vec<Row>>, RepositoryError> {
  let before = revision(directory);
  let epoch = {
    let mut cache = INDEX.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(revision) = before
      && let Some(position) = cache
        .entries
        .iter()
        .position(|item| item.directory == directory && item.revision == revision)
    {
      let item = cache.entries.remove(position).expect("located index");
      let files = item.files.clone();
      cache.entries.push_back(item);
      return Ok(files);
    }
    cache.epoch
  };
  let files = Arc::new(scan(directory)?);
  if let Some(before) = before
    && Some(before) == revision(directory)
    && files.len() <= 8192
  {
    let mut cache = INDEX.lock().unwrap_or_else(|error| error.into_inner());
    if cache.epoch == epoch {
      cache.entries.retain(|item| item.directory != directory);
      cache.entries.push_back(Index {
        directory: directory.into(),
        revision: before,
        files: files.clone(),
      });
      while cache.entries.len() > 64
        || cache.entries.iter().map(|item| item.files.len()).sum::<usize>() > 8192
      {
        cache.entries.pop_front();
      }
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
  pub display_name: Option<String>,
  pub local_path: Option<String>,
  pub visible: bool,
  pub pinned: bool,
  pub folder: Option<String>,
}
#[derive(Serialize)]
pub struct Page {
  pub items: Vec<Item>,
  pub next: Option<usize>,
  pub total: usize,
  pub folders: Vec<super::library::Folder>,
}

fn scan(directory: &Path) -> Result<Vec<Row>, RepositoryError> {
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
    files.push(Row { modified_at, id, linked_blob });
  }
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
  list_in(directory, start, limit, include_hidden, None)
}
pub fn list_in(
  directory: &Path,
  start: usize,
  limit: usize,
  include_hidden: bool,
  folder: Option<&str>,
) -> Result<Page, RepositoryError> {
  // Presentation and organization are one atomic snapshot, never cached by directory mtime.
  // Two metadata writes can share a filesystem timestamp while changing pins or visibility.
  let library = super::library::snapshot(directory)?;
  let folders: Vec<_> = library.folders.into_values().collect();
  let locations = library.locations;
  let default = super::library::Entry::default();
  let presentation = |row: &Row| library.entries.get(&row.id).unwrap_or(&default);
  if let Some(folder) = folder
    && folder != "/"
    && !folders.iter().any(|item| item.id == folder)
  {
    return Err(RepositoryError::not_found());
  }
  let files = indexed(directory)?;
  let mut files: Vec<_> = files
    .iter()
    .filter(|row| {
      (include_hidden || presentation(row).visible)
        && folder
          .is_none_or(|folder| locations.get(&row.id).map(String::as_str).unwrap_or("/") == folder)
    })
    .collect();
  files.sort_unstable_by(|a, b| {
    (presentation(b).pinned, b.modified_at, &b.id).cmp(&(
      presentation(a).pinned,
      a.modified_at,
      &a.id,
    ))
  });
  let total = files.len();
  let mut items = Vec::new();
  for Row { modified_at, id, linked_blob } in files.into_iter().skip(start).take(limit).cloned() {
    let presentation = library.entries.get(&id).unwrap_or(&default);
    let folder = locations.get(&id).cloned();
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
        display_name: presentation.display_name.clone(),
        local_path,
        visible: presentation.visible,
        pinned: presentation.pinned,
        folder,
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
      display_name: presentation.display_name.clone(),
      local_path,
      visible: presentation.visible,
      pinned: presentation.pinned,
      folder,
    });
  }
  let end = start.saturating_add(limit);
  Ok(Page { next: (end < total).then_some(end), items, total, folders })
}
