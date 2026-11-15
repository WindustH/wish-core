//! Library reclamation shares the visibility write lock with show/pin operations.
use super::{RepositoryError, WRITES, is_blob_id, read, write};
use serde::Serialize;
use std::{
  collections::{BTreeSet, HashSet},
  path::Path,
};

#[derive(Default, Serialize)]
pub struct Cleanup {
  pub missing_links: u64,
  pub unused_hidden: u64,
  pub stale_entries: u64,
  pub bytes: u64,
  pub protected: u64,
  pub ids: Vec<String>,
}
pub struct CleanupOptions<'a> {
  pub missing_links: bool,
  pub unused_hidden: bool,
  /// The caller could not establish resource ownership; preserve every candidate.
  pub retain_all: bool,
  /// Restrict application to the reviewed set; eligibility is still checked again.
  pub ids: Option<&'a HashSet<String>>,
}

pub fn cleanup(
  directory: &Path,
  retained: &HashSet<String>,
  options: CleanupOptions<'_>,
  apply: bool,
) -> Result<Cleanup, RepositoryError> {
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  let _catalog = apply.then(|| crate::storage::blobs::catalog::Mutation(directory));
  let mut ids = BTreeSet::new();
  match std::fs::read_dir(directory) {
    Ok(entries) => {
      for entry in entries {
        let entry = entry.map_err(RepositoryError::internal)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let id = name.strip_suffix(".json").unwrap_or(&name);
        if is_blob_id(id) && options.ids.is_none_or(|ids| ids.contains(id)) {
          ids.insert(id.to_owned());
        }
      }
    }
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Cleanup::default()),
    Err(error) => return Err(RepositoryError::internal(error)),
  }
  ids.extend(
    library
      .entries
      .keys()
      .chain(library.locations.keys())
      .filter(|id| is_blob_id(id) && options.ids.is_none_or(|ids| ids.contains(*id)))
      .cloned(),
  );
  let mut result = Cleanup::default();
  for id in ids {
    let path = directory.join(&id);
    let description = crate::storage::blobs::description_path(directory, &id);
    let mut paths = Vec::new();
    let mut missing_link = false;
    match std::fs::symlink_metadata(&path) {
      Ok(metadata) if metadata.is_file() => paths.push((path, metadata.len())),
      Ok(_) => continue,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
      Err(error) => return Err(RepositoryError::internal(error)),
    }
    match std::fs::symlink_metadata(&description) {
      Ok(metadata) if metadata.is_file() => {
        if paths.is_empty() {
          let bytes = crate::utils::bounded_file::read_sync(&description, 64 * 1024)
            .map_err(RepositoryError::internal)?;
          let Ok(blob) = serde_json::from_slice::<crate::storage::blobs::Blob>(&bytes) else {
            continue;
          };
          if !blob.linked || blob.id != id {
            continue;
          }
          missing_link = match std::fs::metadata(&blob.path) {
            Ok(metadata) => !metadata.is_file(),
            Err(error)
              if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
              ) =>
            {
              true
            }
            // An inaccessible source might return later. Do not label it a broken link.
            Err(_) => false,
          };
        }
        paths.push((description, metadata.len()));
      }
      Ok(_) => continue,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
      Err(error) => return Err(RepositoryError::internal(error)),
    }
    let broken = options.missing_links && missing_link;
    let hidden =
      options.unused_hidden && !library.entries.get(&id).is_some_and(|entry| entry.visible);
    if !broken && !hidden {
      continue;
    }
    if options.retain_all || retained.contains(&id) {
      result.protected += u64::from(!paths.is_empty());
      continue;
    }
    let stale = paths.is_empty();
    for (path, bytes) in paths {
      result.bytes += bytes;
      if apply {
        match std::fs::remove_file(path) {
          Ok(()) => {}
          Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
          Err(error) => return Err(RepositoryError::internal(error)),
        }
      }
    }
    if stale {
      result.stale_entries += 1
    } else if broken {
      result.missing_links += 1
    } else {
      result.unused_hidden += 1
    }
    if apply {
      library.entries.remove(&id);
      library.locations.remove(&id);
    }
    result.ids.push(id);
  }
  if apply && !result.ids.is_empty() {
    write(directory, &library)?;
  }
  Ok(result)
}

/// History pruning only offers resources released by history; visible membership wins
/// even if it was changed after that selection. Linked originals are never unlinked.
pub fn release_hidden(
  directory: &Path,
  ids: &HashSet<String>,
  apply: bool,
) -> Result<Cleanup, RepositoryError> {
  cleanup(
    directory,
    &HashSet::new(),
    CleanupOptions { missing_links: false, unused_hidden: true, retain_all: false, ids: Some(ids) },
    apply,
  )
}
