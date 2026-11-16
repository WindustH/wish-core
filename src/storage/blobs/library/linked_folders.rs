//! Read-through directory links. Browsing never copies files or mutates the source directory.
use super::*;
use std::path::Component;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
  pub path: String,
  pub name: Option<String>,
  pub parent: Option<String>,
}
#[derive(Serialize)]
pub struct Child {
  pub path: String,
  pub name: String,
  pub display_name: Option<String>,
  pub directory: bool,
  pub byte_count: u64,
}
#[derive(Serialize)]
pub struct Page {
  pub items: Vec<Child>,
  pub next: Option<String>,
}
fn io(error: std::io::Error) -> RepositoryError {
  if error.kind() == std::io::ErrorKind::NotFound {
    RepositoryError::not_found()
  } else {
    RepositoryError::internal(error)
  }
}
pub fn create(directory: &Path, input: Link) -> Result<Folder, RepositoryError> {
  if !Path::new(&input.path).is_absolute() {
    return Err(RepositoryError::invalid("linked folder path must be absolute"));
  }
  let path = std::fs::canonicalize(&input.path).map_err(io)?;
  if !path.is_dir() {
    return Err(RepositoryError::invalid("path is not a directory"));
  }
  let path = path.to_str().ok_or_else(|| RepositoryError::invalid("path must be UTF-8"))?;
  let name = folders::name(
    input
      .name
      .as_deref()
      .unwrap_or_else(|| Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or("Root")),
  )?;
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  folders::parent(&library, input.parent.as_deref())?;
  // Retrying a mixed batch must not duplicate a link or reset its display name.
  if input.name.is_none()
    && let Some(existing) = library
      .folders
      .values()
      .find(|folder| folder.parent == input.parent && folder.linked_path.as_deref() == Some(path))
  {
    return Ok(existing.clone());
  }
  folders::unique(&library, &name, input.parent.as_deref(), None)?;
  if library.folders.len() >= 1000 {
    return Err(RepositoryError::invalid("Library folder limit reached"));
  }
  let folder = Folder {
    id: format!("folder_{}", uuid::Uuid::new_v4()),
    name,
    parent: input.parent,
    linked_path: Some(path.into()),
    aliases: BTreeMap::new(),
  };
  library.folders.insert(folder.id.clone(), folder.clone());
  write(directory, &library)?;
  Ok(folder)
}
fn folder(directory: &Path, id: &str) -> Result<Folder, RepositoryError> {
  snapshot(directory)?
    .folders
    .remove(id)
    .filter(|f| f.linked_path.is_some())
    .ok_or_else(RepositoryError::not_found)
}
fn resolve(folder: &Folder, relative: &str) -> Result<PathBuf, RepositoryError> {
  let path = Path::new(relative);
  if relative.len() > 4096
    || (!relative.is_empty()
      && relative.split('/').any(|part| part.is_empty() || part == "." || part == ".."))
    || path.components().any(|part| !matches!(part, Component::Normal(_)))
  {
    return Err(RepositoryError::invalid("invalid linked folder relative path"));
  }
  let root = Path::new(folder.linked_path.as_deref().ok_or_else(RepositoryError::not_found)?);
  // The saved canonical root is the boundary, including after a symlink is replaced.
  let resolved = std::fs::canonicalize(root.join(path)).map_err(io)?;
  if !resolved.starts_with(root) {
    return Err(RepositoryError::invalid("path leaves linked folder"));
  }
  Ok(resolved)
}
pub fn file(
  directory: &Path,
  id: &str,
  relative: &str,
) -> Result<(String, Option<String>), RepositoryError> {
  let folder = folder(directory, id)?;
  let path = resolve(&folder, relative)?;
  if !path.is_file() {
    return Err(RepositoryError::invalid("path is not a regular file"));
  }
  Ok((
    path.to_str().ok_or_else(|| RepositoryError::invalid("path must be UTF-8"))?.into(),
    folder.aliases.get(relative).cloned(),
  ))
}
pub fn list(
  directory: &Path,
  id: &str,
  relative: &str,
  after: Option<&str>,
  limit: usize,
) -> Result<Page, RepositoryError> {
  if !(1..=200).contains(&limit) {
    return Err(RepositoryError::invalid("limit must be between 1 and 200"));
  }
  let library = snapshot(directory)?;
  let folder = library.folders.get(id).ok_or_else(RepositoryError::not_found)?;
  let path = resolve(folder, relative)?;
  // Filter before counting a page; inaccessible links must not hide later valid children.
  // Retain only a page plus one entry, even when the directory itself is very large.
  let mut children = BTreeMap::new();
  for entry in std::fs::read_dir(path).map_err(io)? {
    let entry = entry.map_err(io)?;
    let Ok(name) = entry.file_name().into_string() else { continue };
    if after.is_some_and(|cursor| name.as_str() <= cursor)
      || (children.len() > limit
        && children.last_key_value().is_some_and(|(last, _)| &name >= last))
    {
      continue;
    }
    let child_path = if relative.is_empty() { name.clone() } else { format!("{relative}/{name}") };
    let Ok(path) = resolve(folder, &child_path) else { continue };
    let Ok(metadata) = std::fs::metadata(&path) else { continue };
    if !metadata.is_file() && !metadata.is_dir() {
      continue;
    }
    let display_name = path
      .to_str()
      .filter(|_| metadata.is_file())
      .and_then(|path| library.entries.get(&crate::storage::blobs::linked::id_for_path(path)))
      .and_then(|entry| entry.display_name.clone())
      .or_else(|| folder.aliases.get(&child_path).cloned());
    children.insert(
      name.clone(),
      Child {
        display_name,
        path: child_path,
        name,
        directory: metadata.is_dir(),
        byte_count: metadata.len(),
      },
    );
    if children.len() > limit + 1 {
      children.pop_last();
    }
  }
  let more = children.len() > limit;
  if more {
    children.pop_last();
  }
  let next = more.then(|| children.last_key_value().map(|(name, _)| name.clone())).flatten();
  Ok(Page { items: children.into_values().collect(), next })
}

pub fn rename(
  directory: &Path,
  id: &str,
  relative: &str,
  name: &str,
) -> Result<(), RepositoryError> {
  let name = display_name(name)?;
  if relative.is_empty() {
    return Err(RepositoryError::invalid("supply a child path"));
  }
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  let folder = library.folders.get_mut(id).ok_or_else(RepositoryError::not_found)?;
  let path = resolve(folder, relative)?;
  if !folder.aliases.contains_key(relative) && folder.aliases.len() >= 2000 {
    return Err(RepositoryError::invalid("linked folder display name limit reached"));
  }
  folder.aliases.insert(relative.to_owned(), name.clone());
  if path.is_file() {
    let path = path.to_str().ok_or_else(|| RepositoryError::invalid("path must be UTF-8"))?;
    library
      .entries
      .entry(crate::storage::blobs::linked::id_for_path(path))
      .or_default()
      .display_name = Some(name);
  }
  write(directory, &library)
}
