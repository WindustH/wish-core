use super::{Storage, StorageError, StoredValue};
use std::{marker::PhantomData, sync::Arc};

/// Internal cache page size, not a limit on the number of items a read may return.
pub(super) const PAGE_SIZE: u64 = 128;
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ListId(pub String);

#[derive(Clone, Debug)]
pub struct Page<T> {
  pub start: u64,
  /// The range's items. Released positions are left out, so a page of a list that has had
  /// positions released may hold fewer items than its range; continue from `end`.
  pub items: Vec<Arc<T>>,
  /// The position after this page's range.
  pub end: u64,
  /// Next position, if another page exists at the time of this read.
  pub next: Option<u64>,
}

/// A typed, lazy, read-only list handle. Opening one loads nothing; each read is a transaction of
/// its own. Lists change only inside [`Storage::transaction`], so the domain objects that own them
/// keep their invariants.
pub struct ReadList<T> {
  storage: Storage,
  id: ListId,
  marker: PhantomData<fn() -> T>,
}
impl<T> Clone for ReadList<T> {
  fn clone(&self) -> Self {
    Self { storage: self.storage.clone(), id: self.id.clone(), marker: PhantomData }
  }
}
impl<T: StoredValue> ReadList<T> {
  pub(super) fn new(storage: Storage, id: ListId) -> Self {
    Self { storage, id, marker: PhantomData }
  }
  pub fn len(&self) -> Result<u64, StorageError> {
    let id = self.id.clone();
    self.storage.transaction(move |tx| tx.list_len::<T>(&id))
  }
  pub fn get(&self, position: u64) -> Result<Option<Arc<T>>, StorageError> {
    let id = self.id.clone();
    self.storage.transaction(move |tx| tx.get_item::<T>(&id, position))
  }
  /// Reads up to `limit` items across cache pages. `limit` must be positive.
  pub fn read_page(&self, start: u64, limit: usize) -> Result<Page<T>, StorageError> {
    let id = self.id.clone();
    self.storage.transaction(move |tx| tx.read_page::<T>(&id, start, limit))
  }
  /// Every item, in one transaction. Suits a list that fits in memory.
  pub fn read_all(&self) -> Result<Vec<Arc<T>>, StorageError> {
    let id = self.id.clone();
    self.storage.transaction(move |tx| tx.read_from::<T>(&id, 0))
  }
  /// The pages from `start` to the end of the list, each read in a transaction of its own, so a
  /// long list is streamed rather than held. The end is the list's end as each read sees it.
  pub fn pages(&self, start: u64) -> Pages<T> {
    Pages { list: self.clone(), next: Some(start) }
  }
}

/// See [`ReadList::pages`].
pub struct Pages<T> {
  list: ReadList<T>,
  next: Option<u64>,
}
impl<T: StoredValue> Iterator for Pages<T> {
  type Item = Result<Page<T>, StorageError>;
  fn next(&mut self) -> Option<Self::Item> {
    let start = self.next.take()?;
    match self.list.read_page(start, PAGE_SIZE as usize) {
      // A start at the end reads an empty range: there is no page left.
      Ok(page) if page.start == page.end => None,
      Ok(page) => {
        self.next = page.next;
        Some(Ok(page))
      }
      Err(error) => Some(Err(error)),
    }
  }
}
