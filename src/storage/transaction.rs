use super::cache::{Cache, CacheKey, CachedValue};
use super::{ListId, PAGE_SIZE, Page, StorageError, StoredValue};
use rusqlite::{OptionalExtension, params};
use std::{any::type_name, borrow::Cow, collections::HashSet, sync::Arc};

// SQLite kinds were written when the session engine was a `wish_core` library. Keep that
// namespace stable now that the same types live in the `wish` executable.
fn stored_kind<T>() -> Cow<'static, str> {
  let name = type_name::<T>();
  let crate_name = module_path!().split("::").next().unwrap();
  let Some(path) = name.strip_prefix(crate_name).and_then(|rest| rest.strip_prefix("::")) else {
    return Cow::Borrowed(name);
  };
  if crate_name == "wish_core"
    || !["executor::", "protocol::", "session::", "storage::", "tool::", "transport::", "utils::"]
      .iter()
      .any(|module| path.starts_with(module))
  {
    Cow::Borrowed(name)
  } else {
    Cow::Owned(format!("wish_core::{path}"))
  }
}

#[cfg(test)]
mod tests {
  use super::stored_kind;

  #[test]
  fn executable_keeps_existing_session_kind_names() {
    assert_eq!(
      stored_kind::<crate::session::SessionConfig>(),
      "wish_core::session::config::SessionConfig"
    );
  }
}

pub struct Transaction<'a> {
  pub(crate) sql: rusqlite::Transaction<'a>,
  pub(crate) cache: &'a mut Cache,
  pub(crate) dirty: HashSet<CacheKey>,
}
impl Transaction<'_> {
  fn load_cached<T: Send + Sync + 'static>(&mut self, key: &CacheKey) -> Option<Arc<T>> {
    if self.dirty.contains(key) {
      return None;
    }
    self.cache.get(key)?.value.downcast::<T>().ok()
  }
  fn cache_value<T: Send + Sync + 'static>(&mut self, key: CacheKey, value: Arc<T>, bytes: usize) {
    if !self.dirty.contains(&key) {
      self.cache.insert(key, CachedValue { value, weight: bytes.saturating_add(128) as u64 });
    }
  }
  pub fn create_object<T: StoredValue>(
    &mut self,
    name: &str,
    value: &T,
  ) -> Result<(), StorageError> {
    let bytes = serde_json::to_vec(value)?;
    let changed = self.sql.execute(
      "INSERT OR IGNORE INTO wish_objects(name,kind,value) VALUES(?1,?2,?3)",
      params![name, stored_kind::<T>().as_ref(), bytes],
    )?;
    if changed == 0 {
      return Err(StorageError::AlreadyExists(name.into()));
    }
    self.dirty.insert(CacheKey::Object(name.into()));
    Ok(())
  }
  pub fn load_object<T: StoredValue>(&mut self, name: &str) -> Result<Arc<T>, StorageError> {
    let key = CacheKey::Object(name.into());
    if let Some(value) = self.load_cached::<T>(&key) {
      return Ok(value);
    }
    let (kind, bytes): (String, Vec<u8>) = self
      .sql
      .query_row("SELECT kind,value FROM wish_objects WHERE name=?1", [name], |row| {
        Ok((row.get(0)?, row.get(1)?))
      })
      .optional()?
      .ok_or_else(|| StorageError::NotFound(name.into()))?;
    if kind != stored_kind::<T>() {
      return Err(StorageError::TypeMismatch(name.into()));
    }
    let value = Arc::new(serde_json::from_slice::<T>(&bytes)?);
    self.cache_value(key, value.clone(), bytes.len());
    Ok(value)
  }
  pub fn save_object<T: StoredValue>(&mut self, name: &str, value: &T) -> Result<(), StorageError> {
    let _ = self.load_object::<T>(name)?;
    let bytes = serde_json::to_vec(value)?;
    self.sql.execute("UPDATE wish_objects SET value=?1 WHERE name=?2", params![bytes, name])?;
    self.dirty.insert(CacheKey::Object(name.into()));
    Ok(())
  }
  pub fn create_list<T: StoredValue>(&mut self, list: &ListId) -> Result<(), StorageError> {
    let changed = self.sql.execute(
      "INSERT OR IGNORE INTO wish_lists(name,kind,length) VALUES(?1,?2,0)",
      params![list.0, stored_kind::<T>().as_ref()],
    )?;
    if changed == 0 {
      return Err(StorageError::AlreadyExists(list.0.clone()));
    }
    self.sql.execute("INSERT INTO wish_list_keys(name) VALUES(?1)", [&list.0])?;
    Ok(())
  }
  pub fn list_len<T: StoredValue>(&self, list: &ListId) -> Result<u64, StorageError> {
    let (kind, length): (String, i64) = self
      .sql
      .query_row("SELECT kind,length FROM wish_lists WHERE name=?1", [&list.0], |row| {
        Ok((row.get(0)?, row.get(1)?))
      })
      .optional()?
      .ok_or_else(|| StorageError::NotFound(list.0.clone()))?;
    if kind != stored_kind::<T>() {
      return Err(StorageError::TypeMismatch(list.0.clone()));
    }
    u64::try_from(length).map_err(|_| StorageError::Corrupt("negative length".into()))
  }
  pub fn get_item<T: StoredValue>(
    &mut self,
    list: &ListId,
    position: u64,
  ) -> Result<Option<Arc<T>>, StorageError> {
    if position >= self.list_len::<T>(list)? {
      return Ok(None);
    }
    let key = CacheKey::Item(list.0.clone(), position);
    if let Some(value) = self.load_cached(&key) {
      return Ok(Some(value));
    }
    let bytes: Vec<u8> = self.sql.query_row(
      "SELECT value FROM wish_items WHERE list=(SELECT id FROM wish_list_keys WHERE name=?1) AND position=?2",
      params![list.0, position as i64],
      |row| row.get(0),
    )?;
    let value = Arc::new(serde_json::from_slice::<T>(&bytes)?);
    self.cache_value(key, value.clone(), bytes.len());
    Ok(Some(value))
  }
  pub fn append_item<T: StoredValue>(
    &mut self,
    list: &ListId,
    value: &T,
  ) -> Result<u64, StorageError> {
    self.append_items(list, std::slice::from_ref(value))
  }
  /// Append a batch and return its starting position. Empty batches return the current length.
  /// Reuses one INSERT statement and updates the list length once.
  pub fn append_items<T: StoredValue>(
    &mut self,
    list: &ListId,
    values: &[T],
  ) -> Result<u64, StorageError> {
    let start = self.list_len::<T>(list)?;
    let end = start
      .checked_add(values.len() as u64)
      .filter(|end| *end <= i64::MAX as u64)
      .ok_or(StorageError::InvalidRange)?;
    if values.is_empty() {
      return Ok(start);
    }
    // Encode before modifying the list, so encoding errors never leave a partial batch.
    let encoded = values.iter().map(serde_json::to_vec).collect::<Result<Vec<_>, _>>()?;
    let mut statement =
      self.sql.prepare("INSERT INTO wish_items(list,position,value) VALUES((SELECT id FROM wish_list_keys WHERE name=?1),?2,?3)")?;
    for (offset, bytes) in encoded.iter().enumerate() {
      statement.execute(params![list.0, (start + offset as u64) as i64, bytes])?;
    }
    drop(statement);
    self
      .sql
      .execute("UPDATE wish_lists SET length=?1 WHERE name=?2", params![end as i64, list.0])?;
    for position in start..end {
      self.mark_item_changed(list, position);
    }
    Ok(start)
  }
  pub fn set_item<T: StoredValue>(
    &mut self,
    list: &ListId,
    position: u64,
    value: &T,
  ) -> Result<(), StorageError> {
    if position >= self.list_len::<T>(list)? {
      return Err(StorageError::InvalidRange);
    }
    let bytes = serde_json::to_vec(value)?;
    self.sql.execute(
      "UPDATE wish_items SET value=?1 WHERE list=(SELECT id FROM wish_list_keys WHERE name=?2) AND position=?3",
      params![bytes, list.0, position as i64],
    )?;
    self.mark_item_changed(list, position);
    Ok(())
  }
  /// Remove one list position and shift the suffix left in this transaction.
  pub fn remove_item<T: StoredValue>(
    &mut self,
    list: &ListId,
    position: u64,
  ) -> Result<(), StorageError> {
    let length = self.list_len::<T>(list)?;
    if position >= length {
      return Err(StorageError::InvalidRange);
    }
    self.sql.execute(
      "DELETE FROM wish_items WHERE list=(SELECT id FROM wish_list_keys WHERE name=?1) AND position=?2",
      params![list.0, position as i64],
    )?;
    let mut statement = self
      .sql
      .prepare("UPDATE wish_items SET position=position-1 WHERE list=(SELECT id FROM wish_list_keys WHERE name=?1) AND position=?2")?;
    for source in position + 1..length {
      statement.execute(params![list.0, source as i64])?;
    }
    drop(statement);
    self.sql.execute("UPDATE wish_lists SET length=length-1 WHERE name=?1", [&list.0])?;
    for changed in position..length {
      self.mark_item_changed(list, changed);
    }
    Ok(())
  }
  fn mark_item_changed(&mut self, list: &ListId, position: u64) {
    self.dirty.insert(CacheKey::Item(list.0.clone(), position));
    self.dirty.insert(CacheKey::Page(list.0.clone(), position / PAGE_SIZE));
  }
  /// Reads up to `limit` values across cache pages using indexed position ranges, never OFFSET scans.
  /// `limit` must be positive; the result is truncated at the end of the list.
  pub fn read_page<T: StoredValue>(
    &mut self,
    list: &ListId,
    start: u64,
    limit: usize,
  ) -> Result<Page<T>, StorageError> {
    if limit == 0 {
      return Err(StorageError::InvalidRange);
    }
    let length = self.list_len::<T>(list)?;
    if start > length {
      return Err(StorageError::InvalidRange);
    }
    let end = start.saturating_add(limit as u64).min(length);
    let mut items = Vec::with_capacity((end - start) as usize);
    let mut position = start;
    while position < end {
      let page_number = position / PAGE_SIZE;
      let page_start = page_number * PAGE_SIZE;
      let key = CacheKey::Page(list.0.clone(), page_number);
      let page = if let Some(page) = self.load_cached::<Vec<Arc<T>>>(&key) {
        page
      } else {
        let mut statement = self.sql.prepare("SELECT position,value FROM wish_items WHERE list=(SELECT id FROM wish_list_keys WHERE name=?1) AND position>=?2 AND position<?3 ORDER BY position")?;
        let rows = statement.query_map(
          params![
            list.0,
            page_start as i64,
            page_start.saturating_add(PAGE_SIZE).min(i64::MAX as u64) as i64
          ],
          |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )?;
        let mut values = Vec::new();
        let mut weight = 0usize;
        for row in rows {
          let (index, bytes) = row?;
          if index < 0 || index as u64 != page_start + values.len() as u64 {
            return Err(StorageError::Corrupt("non-contiguous list".into()));
          }
          weight = weight.saturating_add(bytes.len());
          values.push(Arc::new(serde_json::from_slice::<T>(&bytes)?));
        }
        drop(statement);
        let page = Arc::new(values);
        self.cache_value(key, page.clone(), weight);
        page
      };
      let count = (end - position).min(PAGE_SIZE - (position - page_start)) as usize;
      let offset = (position - page_start) as usize;
      if offset + count > page.len() {
        return Err(StorageError::Corrupt("short list page".into()));
      }
      items.extend_from_slice(&page[offset..offset + count]);
      position += count as u64;
    }
    Ok(Page { start, items, next: (end < length).then_some(end) })
  }
}

impl Transaction<'_> {
  /// Remove an object's namespace, including its lists and derived history indexes.
  /// Namespace matching is literal and only includes descendants separated by `/`.
  pub fn delete_namespace(&mut self, namespace: &str) -> Result<(), StorageError> {
    let prefix = format!("{namespace}/");
    // Eviction before commit is safe on rollback: it only discards cached values.
    self.cache.clear();
    self
      .sql
      .execute("DELETE FROM wish_history_index WHERE list IN (SELECT id FROM wish_list_keys WHERE substr(name,1,length(?1))=?1)", [&prefix])?;
    self.sql.execute("DELETE FROM wish_items WHERE list IN (SELECT id FROM wish_list_keys WHERE substr(name,1,length(?1))=?1)", [&prefix])?;
    self.sql.execute("DELETE FROM wish_list_keys WHERE substr(name,1,length(?1))=?1", [&prefix])?;
    self.sql.execute("DELETE FROM wish_lists WHERE substr(name,1,length(?1))=?1", [&prefix])?;
    self.sql.execute(
      "DELETE FROM wish_objects WHERE name=?1 OR substr(name,1,length(?2))=?2",
      rusqlite::params![namespace, prefix],
    )?;
    Ok(())
  }
}
