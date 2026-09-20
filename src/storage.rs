//! Typed SQLite objects and paged lists. Transactions publish cache changes only after commit.
mod cache;
mod database;
mod list;
mod object;
mod transaction;

pub(crate) use database::OwnerGuard;
pub use database::{Storage, StorageOptions};
pub use list::{ListId, PAGE_SIZE, Page, ReadList, StoredList};
pub use object::StoredObject;
pub use transaction::Transaction;

use serde::{Serialize, de::DeserializeOwned};
pub trait StoredValue: Clone + Serialize + DeserializeOwned + Send + Sync + 'static {}
impl<T: Clone + Serialize + DeserializeOwned + Send + Sync + 'static> StoredValue for T {}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
  #[error("SQLite: {0}")]
  Sqlite(#[from] rusqlite::Error),
  #[error("stored value encoding: {0}")]
  Encoding(#[from] serde_json::Error),
  #[error("could not start storage worker: {0}")]
  StartWorker(std::io::Error),
  #[error("storage worker is closed")]
  Closed,
  #[error("WAL checkpoint could not complete because the database is busy")]
  CheckpointBusy,
  #[error("storage operations cannot be nested; use the transaction provided to the closure")]
  NestedOperation,
  #[error("storage operation panicked and was rolled back")]
  OperationPanicked,
  #[error("session already has an owner: {0}")]
  AlreadyOpen(String),
  #[error("stored object or list does not exist: {0}")]
  NotFound(String),
  #[error("stored object or list already exists: {0}")]
  AlreadyExists(String),
  #[error("stored value has a different type: {0}")]
  TypeMismatch(String),
  #[error("invalid storage position or page limit")]
  InvalidRange,
  #[error("unsupported database schema version: {0}")]
  SchemaVersion(i64),
  #[error("storage data is inconsistent: {0}")]
  Corrupt(String),
}
