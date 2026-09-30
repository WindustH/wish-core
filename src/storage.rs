//! Typed SQLite objects and paged lists. Transactions publish cache changes only after commit.

/// SQL for the interned key of the list named by parameter 1: items and history-index rows refer
/// to a list by that key.
macro_rules! list_key {
  () => {
    "(SELECT id FROM wish_list_keys WHERE name=?1)"
  };
}

mod cache;
mod database;
pub(crate) mod history_index;
mod list;
mod transaction;

pub(crate) use database::OwnerGuard;
pub use database::{NamespaceUsage, Storage, StorageOptions};
pub use list::{ListId, Page, ReadList};
pub use transaction::Transaction;

use serde::{Serialize, de::DeserializeOwned};

/// A value storage can keep: an object, or the items of a list.
pub trait StoredValue: Clone + Serialize + DeserializeOwned + Send + Sync + 'static {
  /// The kind a value is stored under, beside it in the database. A value is read back only as the
  /// kind it was written as, so this string is part of the database format: it stays the same when
  /// the type is renamed or moved, and changing it needs a migration.
  const KIND: &'static str;
}

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
