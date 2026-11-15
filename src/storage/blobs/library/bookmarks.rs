//! Durable, owner-local pointers into conversation history; never sent to the model.
use super::*;

#[derive(Clone, Serialize, Deserialize)]
pub struct Bookmark {
  pub id: String,
  pub seq: u64,
  pub block: Option<usize>,
  pub start: Option<usize>,
  pub end: Option<usize>,
  pub text: String,
  pub name: String,
  pub created_at: u64,
}
pub fn list(directory: &Path) -> Result<Vec<Bookmark>, RepositoryError> {
  let mut items: Vec<_> = snapshot(directory)?.bookmarks.into_values().collect();
  items.sort_by(|a, b| (b.created_at, &b.id).cmp(&(a.created_at, &a.id)));
  Ok(items)
}
pub fn add(directory: &Path, bookmark: Bookmark) -> Result<Bookmark, RepositoryError> {
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  if let Some(existing) = library.bookmarks.values().find(|b| {
    b.seq == bookmark.seq
      && b.block == bookmark.block
      && b.start == bookmark.start
      && b.end == bookmark.end
  }) {
    return Ok(existing.clone());
  }
  if library.bookmarks.len() >= 1000 {
    return Err(RepositoryError::invalid("Library bookmark limit reached"));
  }
  library.bookmarks.insert(bookmark.id.clone(), bookmark.clone());
  write(directory, &library)?;
  Ok(bookmark)
}
pub fn rename(directory: &Path, id: &str, name: &str) -> Result<Bookmark, RepositoryError> {
  let name = display_name(name)?;
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  let bookmark = library.bookmarks.get_mut(id).ok_or_else(RepositoryError::not_found)?;
  bookmark.name = name;
  let result = bookmark.clone();
  write(directory, &library)?;
  Ok(result)
}
pub fn delete(directory: &Path, id: &str) -> Result<(), RepositoryError> {
  let lock = WRITES.get(directory.to_path_buf());
  let _guard = lock.lock().map_err(RepositoryError::internal)?;
  let mut library = read(directory)?;
  if library.bookmarks.remove(id).is_none() {
    return Err(RepositoryError::not_found());
  }
  write(directory, &library)
}
