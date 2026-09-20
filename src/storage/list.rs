use super::{Storage, StorageError, StoredValue};
use std::{marker::PhantomData, sync::Arc};

pub const PAGE_SIZE: u64 = 128;
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ListId(pub String);

#[derive(Clone, Debug)]
pub struct Page<T> {
  pub start: u64,
  pub items: Vec<Arc<T>>,
  /// Next position, if another page exists at the time of this read.
  pub next: Option<u64>,
}

/// A typed, lazy list handle. Loading a handle never loads its elements.
pub struct StoredList<T> {
  storage: Storage,
  id: ListId,
  marker: PhantomData<fn() -> T>,
}
impl<T> Clone for StoredList<T> {
  fn clone(&self) -> Self {
    Self { storage: self.storage.clone(), id: self.id.clone(), marker: PhantomData }
  }
}
impl<T: StoredValue> StoredList<T> {
  pub(crate) fn new(storage: Storage, id: ListId) -> Self {
    Self { storage, id, marker: PhantomData }
  }
  pub fn read_only(&self) -> ReadList<T> {
    ReadList(self.clone())
  }
  pub fn get_id(&self) -> &ListId {
    &self.id
  }
  pub fn len(&self) -> Result<u64, StorageError> {
    let id = self.id.clone();
    self.storage.transaction(move |tx| tx.list_len::<T>(&id))
  }
  pub fn is_empty(&self) -> Result<bool, StorageError> {
    Ok(self.len()? == 0)
  }
  pub fn get(&self, position: u64) -> Result<Option<Arc<T>>, StorageError> {
    let id = self.id.clone();
    self.storage.transaction(move |tx| tx.get_item::<T>(&id, position))
  }
  pub fn read_page(&self, start: u64, limit: usize) -> Result<Page<T>, StorageError> {
    let id = self.id.clone();
    self.storage.transaction(move |tx| tx.read_page::<T>(&id, start, limit))
  }
  pub fn append(&self, value: &T) -> Result<u64, StorageError> {
    let id = self.id.clone();
    let value = value.clone();
    self.storage.transaction(move |tx| tx.append_item(&id, &value))
  }
  pub fn set(&self, position: u64, value: &T) -> Result<(), StorageError> {
    let id = self.id.clone();
    let value = value.clone();
    self.storage.transaction(move |tx| tx.set_item(&id, position, &value))
  }
}

/// A read-only view used by domain objects to protect their mutation invariants.
pub struct ReadList<T>(StoredList<T>);
impl<T> Clone for ReadList<T> {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}
impl<T: StoredValue> ReadList<T> {
  pub fn get_id(&self) -> &ListId {
    self.0.get_id()
  }
  pub fn len(&self) -> Result<u64, StorageError> {
    self.0.len()
  }
  pub fn is_empty(&self) -> Result<bool, StorageError> {
    self.0.is_empty()
  }
  pub fn get(&self, position: u64) -> Result<Option<Arc<T>>, StorageError> {
    self.0.get(position)
  }
  pub fn read_page(&self, start: u64, limit: usize) -> Result<Page<T>, StorageError> {
    self.0.read_page(start, limit)
  }
}
