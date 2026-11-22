//! The management index, `management.sqlite`: what the server keeps beside the engine's storage.
//!
//! - `sessions`: a record per session, for the session list;
//! - `folders`: the folders the list is arranged in, and where everything is;
//! - `usage`: every session's model calls and one-second stream samples, for the statistics.
pub mod archives;
mod folders;
pub mod sampling;
mod sessions;
mod usage;

pub use folders::FolderRecord;
pub use sessions::{ConversationEntry, ConversationRow, RecordPage, SessionFilter};
pub use usage::{IndexedModelCall, StreamAggregate, StreamSample, Usage, UsageBucket};

use crate::storage::repository_error::RepositoryError;
use crate::storage::{DatabaseShape, database_shape};
use rusqlite::Connection;
use std::{
  path::Path,
  sync::{Mutex, atomic::AtomicU64},
};

pub struct ManagementStore {
  db: Mutex<Connection>,
  /// The most stream samples kept, as the configuration last said; 0 keeps every one.
  sample_limit: AtomicU64,
}

/// The tables, as a new file gets them; format migrations bring an older file here.
const SCHEMA: &str = "
  CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, updated_at INTEGER NOT NULL, record TEXT NOT NULL, folder TEXT, pinned INTEGER NOT NULL DEFAULT 0);
  CREATE INDEX IF NOT EXISTS sessions_updated ON sessions(updated_at DESC,id);
  CREATE INDEX IF NOT EXISTS sessions_folder ON sessions(folder,pinned DESC,updated_at DESC,id);
  CREATE TABLE IF NOT EXISTS folders(id TEXT PRIMARY KEY, name TEXT NOT NULL, parent TEXT, pinned INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL);
  CREATE INDEX IF NOT EXISTS folders_parent ON folders(parent,pinned DESC,name COLLATE NOCASE);
  CREATE TABLE IF NOT EXISTS calls(session TEXT NOT NULL,position INTEGER NOT NULL,provider TEXT NOT NULL,model TEXT NOT NULL,started_at INTEGER NOT NULL,record TEXT NOT NULL,PRIMARY KEY(session,position));
  CREATE INDEX IF NOT EXISTS calls_time ON calls(started_at);
  CREATE INDEX IF NOT EXISTS calls_session_time ON calls(session,started_at);
  CREATE TABLE IF NOT EXISTS stream_samples(attempt_id TEXT NOT NULL,session TEXT,provider TEXT NOT NULL,model TEXT NOT NULL,at_ms INTEGER NOT NULL,duration_ms INTEGER NOT NULL,output_bytes INTEGER NOT NULL,PRIMARY KEY(attempt_id,at_ms));
  CREATE INDEX IF NOT EXISTS stream_samples_time ON stream_samples(at_ms);
  CREATE INDEX IF NOT EXISTS stream_samples_session_time ON stream_samples(session,at_ms);
";

impl ManagementStore {
  fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, RepositoryError> {
    self.db.lock().map_err(|_| RepositoryError::internal("management database lock poisoned"))
  }

  pub fn open(path: &Path) -> Result<Self, RepositoryError> {
    let db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    // Incremental auto-vacuum lets what is deleted leave the file ([`shrink`]). It takes hold on a
    // new file; format migration 8 rebuilds an older one in it.
    db.execute_batch("PRAGMA auto_vacuum=INCREMENTAL; PRAGMA journal_mode=WAL;")?;
    db.execute_batch(SCHEMA)?;
    db.execute_batch(archives::SCHEMA)?;
    Ok(Self { db: Mutex::new(db), sample_limit: AtomicU64::new(0) })
  }
  /// How the index's file is used: its pages by table and index, and what is free.
  pub fn shape(&self) -> Result<DatabaseShape, RepositoryError> {
    Ok(database_shape(&*self.lock()?)?)
  }
}

/// Cuts the free pages off the end of the file and empties the write-ahead log, so what was
/// deleted leaves the disk. A file not yet in incremental auto-vacuum mode keeps its free pages
/// for reuse.
fn shrink(db: &Connection) -> rusqlite::Result<()> {
  {
    // Each step frees one page and returns a row.
    let mut freeing = db.prepare("PRAGMA incremental_vacuum")?;
    let mut pages = freeing.query([])?;
    while pages.next()?.is_some() {}
  }
  db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
}
