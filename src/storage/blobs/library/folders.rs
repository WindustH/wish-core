//! Owner-local virtual folders. Moving links never relocates their source files.
use super::*;
use std::collections::HashSet;

#[derive(Clone, Deserialize, Serialize)]
pub struct Folder {
  pub id: String,
  pub name: String,
  pub parent: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub linked_path: Option<String>,
  #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
  pub(super) aliases: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderInput {
  pub name: String,
  pub parent: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderMove {
  pub ids: Vec<String>,
  pub folder: Option<String>,
}
pub(super) fn name(value: &str) -> Result<String, RepositoryError> {
  let value = value.trim();
  if value.is_empty()
    || value.len() > 255
    || value == "."
    || value == ".."
    || value.chars().any(|c| c.is_control() || matches!(c, '/' | '\\'))
  {
    return Err(RepositoryError::invalid(
      "folder name must be 1–255 bytes without slashes or control characters",
    ));
  }
  Ok(value.into())
}
pub(super) fn parent(library: &Library, value: Option<&str>) -> Result<(), RepositoryError> {
  if value.is_some_and(|id| !library.folders.contains_key(id)) {
    return Err(RepositoryError::not_found());
  }
  if value.and_then(|id| library.folders.get(id)).is_some_and(|folder| folder.linked_path.is_some())
  {
    return Err(RepositoryError::invalid("linked folders are read-only"));
  }
  Ok(())
}
pub(super) fn unique(
  library: &Library,
  name: &str,
  parent: Option<&str>,
  except: Option<&str>,
) -> Result<(), RepositoryError> {
  if library.folders.values().any(|folder| {
    Some(folder.id.as_str()) != except && folder.parent.as_deref() == parent && folder.name == name
  }) {
    return Err(RepositoryError::invalid("a folder with this name already exists here"));
  }
  Ok(())
}
pub fn create_folder(directory: &Path, input: FolderInput) -> Result<Folder, RepositoryError> {
  let name = name(&input.name)?;
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  parent(&library, input.parent.as_deref())?;
  unique(&library, &name, input.parent.as_deref(), None)?;
  if library.folders.len() >= 1000 {
    return Err(RepositoryError::invalid("Library folder limit reached"));
  }
  let folder = Folder {
    id: format!("folder_{}", uuid::Uuid::new_v4()),
    name,
    parent: input.parent,
    linked_path: None,
    aliases: BTreeMap::new(),
  };
  library.folders.insert(folder.id.clone(), folder.clone());
  write(directory, &library)?;
  Ok(folder)
}
pub fn rename_folder(directory: &Path, id: &str, value: &str) -> Result<Folder, RepositoryError> {
  let name = name(value)?;
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  let folder = library.folders.get(id).ok_or_else(RepositoryError::not_found)?;
  unique(&library, &name, folder.parent.as_deref(), Some(id))?;
  let folder = library.folders.get_mut(id).expect("checked folder");
  folder.name = name;
  let result = folder.clone();
  write(directory, &library)?;
  Ok(result)
}
pub fn move_items(directory: &Path, input: FolderMove) -> Result<(), RepositoryError> {
  if input.ids.is_empty() || input.ids.len() > 2000 {
    return Err(RepositoryError::invalid("move between 1 and 2000 Library items"));
  }
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  parent(&library, input.folder.as_deref())?;
  let ids: HashSet<_> = input.ids.into_iter().collect();
  let mut cursor = input.folder.as_deref();
  let mut seen = HashSet::new();
  while let Some(id) = cursor {
    if ids.contains(id) || !seen.insert(id) {
      return Err(RepositoryError::invalid("a folder cannot be moved inside itself"));
    }
    cursor = library.folders.get(id).and_then(|folder| folder.parent.as_deref());
  }
  let mut names = HashSet::new();
  for id in &ids {
    if let Some(folder) = library.folders.get(id) {
      unique(&library, &folder.name, input.folder.as_deref(), Some(id))?;
      if !names.insert(&folder.name) {
        return Err(RepositoryError::invalid("duplicate folder names"));
      }
    } else if !super::super::catalog::contains(directory, id)? {
      return Err(RepositoryError::not_found());
    }
  }
  for id in ids {
    if let Some(folder) = library.folders.get_mut(&id) {
      folder.parent.clone_from(&input.folder);
    } else if let Some(folder) = &input.folder {
      library.locations.insert(id, folder.clone());
    } else {
      library.locations.remove(&id);
    }
  }
  write(directory, &library)
}
/// Removing an empty organizational folder must never delete content implicitly.
pub fn delete_folder(directory: &Path, id: &str) -> Result<(), RepositoryError> {
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  if !library.folders.contains_key(id) {
    return Err(RepositoryError::not_found());
  }
  if library.locations.values().any(|folder| folder == id)
    || library.folders.values().any(|folder| folder.parent.as_deref() == Some(id))
  {
    return Err(RepositoryError::invalid("move the folder's contents before deleting it"));
  }
  library.folders.remove(id);
  write(directory, &library)
}
