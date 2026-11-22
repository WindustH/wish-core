//! The user's Library is a projection of the blob store, separate from content ownership.
//! Unknown/model-produced blobs stay hidden. Atomic, serialized updates preserve user choices
//! across repeated uploads, concurrent requests and process restarts.

use super::{RepositoryError, is_blob_id};
use serde::{Deserialize, Serialize};
use std::{
  collections::BTreeMap,
  io::Write,
  path::{Path, PathBuf},
  sync::LazyLock,
};

const FILE: &str = ".library.json";
const MAX_BYTES: usize = 16 * 1024 * 1024;
pub mod bookmarks;
mod cleanup;
mod folders;
pub mod linked_folders;
pub use cleanup::{Cleanup, CleanupOptions, cleanup, cleanup_tracked, release_hidden};
pub use folders::{
  Folder, FolderInput, FolderMove, create_folder, delete_folder, move_items, rename_folder,
};
static WRITES: LazyLock<crate::utils::keyed_lock::KeyedLocks<PathBuf>> =
  LazyLock::new(Default::default);

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Entry {
  pub visible: bool,
  pub pinned: bool,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub name: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub display_name: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
  pub visible: Option<bool>,
  pub pinned: Option<bool>,
  pub display_name: Option<String>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Library {
  version: u8,
  pub(super) entries: BTreeMap<String, Entry>,
  #[serde(default)]
  pub(super) folders: BTreeMap<String, Folder>,
  #[serde(default)]
  pub(super) locations: BTreeMap<String, String>,
  #[serde(default)]
  bookmarks: BTreeMap<String, bookmarks::Bookmark>,
}

fn read(directory: &Path) -> Result<Library, RepositoryError> {
  let path = directory.join(FILE);
  let bytes = match crate::utils::bounded_file::read_sync(&path, MAX_BYTES) {
    Ok(bytes) => bytes,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
      return Ok(Library {
        version: 1,
        entries: BTreeMap::new(),
        folders: BTreeMap::new(),
        locations: BTreeMap::new(),
        bookmarks: BTreeMap::new(),
      });
    }
    Err(error) => return Err(RepositoryError::internal(error)),
  };
  let library: Library = serde_json::from_slice(&bytes).map_err(RepositoryError::internal)?;
  if library.version != 1 {
    return Err(RepositoryError::invalid("unsupported Library metadata version"));
  }
  Ok(library)
}

fn write(directory: &Path, library: &Library) -> Result<(), RepositoryError> {
  let bytes = serde_json::to_vec(library).map_err(RepositoryError::internal)?;
  if bytes.len() > MAX_BYTES {
    return Err(RepositoryError::invalid("Library metadata exceeds size limit"));
  }
  std::fs::create_dir_all(directory).map_err(RepositoryError::internal)?;
  let temporary = directory.join(format!(".library-{}.tmp", uuid::Uuid::new_v4()));
  let result = (|| {
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, directory.join(FILE))
  })();
  if result.is_err() {
    let _ = std::fs::remove_file(temporary);
  }
  result.map_err(RepositoryError::internal)
}

/// Only the coordinator knows how to recover legacy membership. Serialize initialization
/// with updates for this owner and never overwrite an existing user's choices.
pub fn initialize(
  directory: &Path,
  recover: impl FnOnce() -> Result<BTreeMap<String, Entry>, RepositoryError>,
) -> Result<(), RepositoryError> {
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  match std::fs::metadata(directory.join(FILE)) {
    Ok(_) => return Ok(()),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
    Err(error) => return Err(RepositoryError::internal(error)),
  }
  write(
    directory,
    &Library {
      version: 1,
      entries: recover()?,
      folders: BTreeMap::new(),
      locations: BTreeMap::new(),
      bookmarks: BTreeMap::new(),
    },
  )
}

/// Read an item's presentation and virtual folder from the same metadata revision.
pub fn item(directory: &Path, id: &str) -> Result<(Entry, Option<String>), RepositoryError> {
  let mut library = snapshot(directory)?;
  Ok((library.entries.remove(id).unwrap_or_default(), library.locations.remove(id)))
}

/// A newly forked owner inherits visibility, names and pins, including files outside context.
pub fn inherit(source: &Path, target: &Path) -> Result<(), RepositoryError> {
  let mut library = {
    let lock = WRITES.get(source.to_path_buf());
    let _guard = lock.lock().map_err(RepositoryError::internal)?;
    read(source)?
  };
  // A fork imports the current context into new history sequences. Old bookmarks must not
  // accidentally navigate to unrelated records in that newly numbered transcript.
  library.bookmarks.clear();
  let lock = WRITES.get(target.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  write(target, &library)
}

/// A direct upload or link is an explicit add. Temporary crops never change an existing choice.
pub fn record_upload(
  directory: &Path,
  id: &str,
  name: Option<String>,
  temporary: bool,
) -> Result<(), RepositoryError> {
  record_file(directory, id, name, temporary, None, None)
}
/// Check a destination before registering a local file; record_in_folder repeats the
/// check under the metadata write lock in case the folder changes concurrently.
pub fn check_parent(directory: &Path, parent: Option<&str>) -> Result<(), RepositoryError> {
  folders::parent(&snapshot(directory)?, parent)
}
pub fn record_in_folder(
  directory: &Path,
  id: &str,
  name: Option<String>,
  parent: Option<String>,
) -> Result<(), RepositoryError> {
  record_file(directory, id, name, false, None, Some(parent))
}
pub fn record_link_preview(
  directory: &Path,
  id: &str,
  name: Option<String>,
  display_name: Option<String>,
) -> Result<(), RepositoryError> {
  record_file(directory, id, name, true, display_name, None)
}
fn record_file(
  directory: &Path,
  id: &str,
  name: Option<String>,
  temporary: bool,
  display_name: Option<String>,
  parent: Option<Option<String>>,
) -> Result<(), RepositoryError> {
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  if let Some(parent) = parent {
    folders::parent(&library, parent.as_deref())?;
    if let Some(parent) = parent {
      library.locations.insert(id.into(), parent);
    } else {
      library.locations.remove(id);
    }
  }
  let entry = library.entries.entry(id.into()).or_default();
  if !temporary {
    entry.visible = true;
  }
  if name.is_some() && (!temporary || entry.name.is_none()) {
    entry.name = name;
  }
  if entry.display_name.is_none() {
    entry.display_name = display_name;
  }
  write(directory, &library)
}

/// Initial inputs and references to already-stored files need no new upload. Preserve a
/// previous hide/pin decision, and exclude reply crops by their durable reference metadata.
pub fn record_input(
  directory: &Path,
  additions: BTreeMap<String, Entry>,
) -> Result<(), RepositoryError> {
  if additions.is_empty() {
    return Ok(());
  }
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  let mut changed = false;
  for (id, entry) in additions {
    match library.entries.entry(id) {
      std::collections::btree_map::Entry::Vacant(slot) => {
        slot.insert(entry);
        changed = true;
      }
      std::collections::btree_map::Entry::Occupied(mut slot)
        if slot.get().name.is_none() && entry.name.is_some() =>
      {
        slot.get_mut().name = entry.name;
        changed = true;
      }
      _ => {}
    }
  }
  if changed {
    write(directory, &library)?;
  }
  Ok(())
}

pub fn update(directory: &Path, id: &str, patch: Patch) -> Result<Entry, RepositoryError> {
  if !is_blob_id(id) {
    return Err(RepositoryError::not_found());
  }
  if patch.visible.is_none() && patch.pinned.is_none() && patch.display_name.is_none() {
    return Err(RepositoryError::invalid("supply visible, pinned or display_name"));
  }
  let display_name = patch.display_name.as_deref().map(display_name).transpose()?;
  if patch.visible == Some(false) && patch.pinned == Some(true) {
    return Err(RepositoryError::invalid("a hidden Library item cannot be pinned"));
  }
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  let entry = library.entries.entry(id.into()).or_default();
  if let Some(name) = display_name {
    entry.display_name = Some(name);
  }
  if let Some(visible) = patch.visible {
    entry.visible = visible;
    if !visible {
      entry.pinned = false;
    }
  }
  if let Some(pinned) = patch.pinned {
    entry.pinned = pinned;
    if pinned {
      entry.visible = true;
    }
  }
  let entry = entry.clone();
  write(directory, &library)?;
  Ok(entry)
}

/// Delete only the owner's stored bytes/descriptor, never a linked source. The caller holds
/// the session's dependency snapshot until this finishes.
pub fn delete(directory: &Path, id: &str) -> Result<(), RepositoryError> {
  if !is_blob_id(id) {
    return Err(RepositoryError::not_found());
  }
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  let _catalog = super::catalog::Mutation(directory);
  let mut removed = false;
  for path in [directory.join(id), super::description_path(directory, id)] {
    match std::fs::remove_file(path) {
      Ok(()) => removed = true,
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
      Err(error) => return Err(RepositoryError::internal(error)),
    }
  }
  if !removed {
    return Err(RepositoryError::not_found());
  }
  library.entries.remove(id);
  library.locations.remove(id);
  write(directory, &library)
}

pub(super) fn snapshot(directory: &Path) -> Result<Library, RepositoryError> {
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  read(directory)
}

pub fn display_name(value: &str) -> Result<String, RepositoryError> {
  let value = value.trim();
  if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
    return Err(RepositoryError::invalid(
      "display name must be 1–1024 bytes without control characters",
    ));
  }
  Ok(value.to_owned())
}
