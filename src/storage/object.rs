use super::{Storage, StorageError, StoredValue};
use std::{marker::PhantomData, sync::Arc};

/// Loads immutable snapshots; save replaces the value on the single storage thread.
pub struct StoredObject<T> {
  storage: Storage,
  name: String,
  marker: PhantomData<fn() -> T>,
}
impl<T: StoredValue> StoredObject<T> {
  pub(crate) fn new(storage: Storage, name: String) -> Self {
    Self { storage, name, marker: PhantomData }
  }
  pub fn load(&self) -> Result<Arc<T>, StorageError> {
    let name = self.name.clone();
    self.storage.transaction(move |tx| tx.load_object(&name))
  }
  pub fn create(&self, value: &T) -> Result<(), StorageError> {
    let name = self.name.clone();
    let value = value.clone();
    self.storage.transaction(move |tx| tx.create_object(&name, &value))
  }
  pub fn save(&self, value: &T) -> Result<(), StorageError> {
    let name = self.name.clone();
    let value = value.clone();
    self.storage.transaction(move |tx| tx.save_object(&name, &value))
  }
}
