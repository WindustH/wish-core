use super::cache::{Cache, CacheKey, CachedValue};
use super::list::PAGE_SIZE;
use super::{ListId, Page, StorageError, StoredValue};
use rusqlite::{OptionalExtension, params};
use std::{collections::HashSet, sync::Arc};

pub struct Transaction<'a> {
  pub(super) sql: rusqlite::Transaction<'a>,
  pub(super) cache: &'a mut Cache,
  pub(super) dirty: HashSet<CacheKey>,
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
      params![name, T::KIND, bytes],
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
    if kind != T::KIND {
      return Err(StorageError::TypeMismatch(name.into()));
    }
    let value = Arc::new(serde_json::from_slice::<T>(&bytes)?);
    self.cache_value(key, value.clone(), bytes.len());
    Ok(value)
  }
  pub fn save_object<T: StoredValue>(&mut self, name: &str, value: &T) -> Result<(), StorageError> {
    let _ = self.load_object::<T>(name)?;
    let bytes = serde_json::to_vec(value)?;
    self.sql.execute("UPDATE wish_objects SET value=?2 WHERE name=?1", params![name, bytes])?;
    self.dirty.insert(CacheKey::Object(name.into()));
    Ok(())
  }
  pub fn create_list<T: StoredValue>(&mut self, list: &ListId) -> Result<(), StorageError> {
    let changed = self.sql.execute(
      "INSERT OR IGNORE INTO wish_lists(name,kind,length) VALUES(?1,?2,0)",
      params![list.0, T::KIND],
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
    if kind != T::KIND {
      return Err(StorageError::TypeMismatch(list.0.clone()));
    }
    u64::try_from(length).map_err(|_| StorageError::Corrupt("negative length".into()))
  }
  /// The item at `position`, or None past the end or where the position was released.
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
      concat!("SELECT value FROM wish_items WHERE list=", list_key!(), " AND position=?2"),
      params![list.0, position as i64],
      |row| row.get(0),
    )?;
    if bytes.is_empty() {
      return Ok(None);
    }
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
    let mut statement = self.sql.prepare(concat!(
      "INSERT INTO wish_items(list,position,value) VALUES(",
      list_key!(),
      ",?2,?3)"
    ))?;
    for (offset, bytes) in encoded.iter().enumerate() {
      statement.execute(params![list.0, (start + offset as u64) as i64, bytes])?;
    }
    drop(statement);
    self
      .sql
      .execute("UPDATE wish_lists SET length=?2 WHERE name=?1", params![list.0, end as i64])?;
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
      concat!("UPDATE wish_items SET value=?3 WHERE list=", list_key!(), " AND position=?2"),
      params![list.0, position as i64, bytes],
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
      concat!("DELETE FROM wish_items WHERE list=", list_key!(), " AND position=?2"),
      params![list.0, position as i64],
    )?;
    let mut statement = self.sql.prepare(concat!(
      "UPDATE wish_items SET position=position-1 WHERE list=",
      list_key!(),
      " AND position=?2"
    ))?;
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
  /// Release the values at `positions` and return how many bytes they held. A released position
  /// keeps its place: the list's length and every other position stay as they are, reads skip it,
  /// and it is never filled again. Positions already released or past the end are ignored.
  pub fn release_items<T: StoredValue>(
    &mut self,
    list: &ListId,
    positions: &[u64],
  ) -> Result<u64, StorageError> {
    let length = self.list_len::<T>(list)?;
    let mut released = 0;
    let mut measure = self.sql.prepare(concat!(
      "SELECT length(value) FROM wish_items WHERE list=",
      list_key!(),
      " AND position=?2"
    ))?;
    let mut clear = self.sql.prepare(concat!(
      "UPDATE wish_items SET value=x'' WHERE list=",
      list_key!(),
      " AND position=?2"
    ))?;
    let mut changed = Vec::new();
    for &position in positions.iter().filter(|position| **position < length) {
      let bytes: i64 = measure.query_row(params![list.0, position as i64], |row| row.get(0))?;
      if bytes > 0 {
        clear.execute(params![list.0, position as i64])?;
        released += bytes as u64;
        changed.push(position);
      }
    }
    drop((measure, clear));
    for position in changed {
      self.mark_item_changed(list, position);
    }
    Ok(released)
  }
  /// Remove a list with its items and history index, returning the bytes its items held.
  pub fn delete_list<T: StoredValue>(&mut self, list: &ListId) -> Result<u64, StorageError> {
    for position in 0..self.list_len::<T>(list)? {
      self.mark_item_changed(list, position);
    }
    let bytes: i64 = self.sql.query_row(
      concat!("SELECT coalesce(sum(length(value)),0) FROM wish_items WHERE list=", list_key!()),
      [&list.0],
      |row| row.get(0),
    )?;
    self.delete_history_rows(list)?;
    self.sql.execute(concat!("DELETE FROM wish_items WHERE list=", list_key!()), [&list.0])?;
    self.sql.execute("DELETE FROM wish_list_keys WHERE name=?1", [&list.0])?;
    self.sql.execute("DELETE FROM wish_lists WHERE name=?1", [&list.0])?;
    Ok(bytes.max(0) as u64)
  }
  fn mark_item_changed(&mut self, list: &ListId, position: u64) {
    self.dirty.insert(CacheKey::Item(list.0.clone(), position));
    self.dirty.insert(CacheKey::Page(list.0.clone(), position / PAGE_SIZE));
  }
  /// Reads up to `limit` positions across cache pages using indexed position ranges, never OFFSET
  /// scans, leaving released positions out. `limit` must be positive; the result is truncated at
  /// the end of the list.
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
      let page = if let Some(page) = self.load_cached::<Vec<Option<Arc<T>>>>(&key) {
        page
      } else {
        let mut statement = self.sql.prepare(concat!(
          "SELECT position,value FROM wish_items WHERE list=",
          list_key!(),
          " AND position>=?2 AND position<?3 ORDER BY position"
        ))?;
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
          // A released position keeps its row, empty, so the list stays contiguous.
          let value = if bytes.is_empty() {
            None
          } else {
            Some(Arc::new(serde_json::from_slice::<T>(&bytes)?))
          };
          values.push(value);
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
      items.extend(page[offset..offset + count].iter().flatten().cloned());
      position += count as u64;
    }
    Ok(Page { start, items, end, next: (end < length).then_some(end) })
  }
  /// The items from `start` to the end the list has now, released positions left out. Suits a list
  /// that fits in memory; visit a long one with [`Transaction::for_each_item`].
  pub fn read_from<T: StoredValue>(
    &mut self,
    list: &ListId,
    start: u64,
  ) -> Result<Vec<Arc<T>>, StorageError> {
    let mut items = Vec::new();
    self.for_each_item::<T, StorageError>(list, start, |_, item| {
      items.push(item.clone());
      Ok(())
    })?;
    Ok(items)
  }
  /// Visits the items from `start` to the end the list has now, a page at a time, released
  /// positions left out. `visit` gets this transaction too; items it appends to the list are not
  /// visited.
  pub fn for_each_item<T: StoredValue, E: From<StorageError>>(
    &mut self,
    list: &ListId,
    start: u64,
    mut visit: impl FnMut(&mut Self, &Arc<T>) -> Result<(), E>,
  ) -> Result<(), E> {
    let end = self.list_len::<T>(list)?;
    let mut position = start;
    while position < end {
      let page = self.read_page::<T>(list, position, (end - position).min(PAGE_SIZE) as usize)?;
      for item in &page.items {
        visit(self, item)?;
      }
      position = page.end;
    }
    Ok(())
  }

  /// Remove an object's namespace, including its lists and derived history indexes.
  /// Namespace matching is literal and only includes descendants separated by `/`.
  pub fn delete_namespace(&mut self, namespace: &str) -> Result<(), StorageError> {
    let prefix = format!("{namespace}/");
    // Eviction before commit is safe on rollback: it only discards cached values.
    self.cache.clear();
    self.delete_namespace_history_rows(&prefix)?;
    self.sql.execute("DELETE FROM wish_items WHERE list IN (SELECT id FROM wish_list_keys WHERE substr(name,1,length(?1))=?1)", [&prefix])?;
    self.sql.execute("DELETE FROM wish_list_keys WHERE substr(name,1,length(?1))=?1", [&prefix])?;
    self.sql.execute("DELETE FROM wish_lists WHERE substr(name,1,length(?1))=?1", [&prefix])?;
    self.sql.execute(
      "DELETE FROM wish_objects WHERE name=?1 OR substr(name,1,length(?2))=?2",
      params![namespace, prefix],
    )?;
    Ok(())
  }
}
