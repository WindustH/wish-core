use super::cache::Cache;
use super::{ListId, ReadList, StorageError, StoredValue, Transaction};
use rusqlite::Connection;
use std::{
  collections::{BTreeMap, HashSet},
  path::{Path, PathBuf},
  sync::{Arc, mpsc},
  thread,
};

#[derive(Clone, Copy, Debug)]
pub struct StorageOptions {
  pub cache_capacity_bytes: u64,
}
impl Default for StorageOptions {
  fn default() -> Self {
    Self { cache_capacity_bytes: 32 * 1024 * 1024 }
  }
}
struct Database {
  connection: Connection,
  cache: Cache,
  owners: HashSet<String>,
}
type Job = Box<dyn FnOnce(&mut Database) + Send>;
enum Command {
  Run(Job),
  Shutdown(mpsc::SyncSender<Result<(), StorageError>>),
}
struct Worker {
  sender: Option<mpsc::Sender<Command>>,
  thread: Option<thread::JoinHandle<()>>,
  id: thread::ThreadId,
}
impl Drop for Worker {
  fn drop(&mut self) {
    self.sender.take();
    if let Some(thread) = self.thread.take()
      && thread.thread().id() != thread::current().id()
    {
      let _ = thread.join();
    }
  }
}
enum Source {
  File(PathBuf),
  Memory,
}

/// The schema this build creates and opens; see [`Database::open`].
const SCHEMA_VERSION: i64 = 4;

/// What one namespace holds; see [`Storage::measure_children`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NamespaceUsage {
  pub bytes: u64,
  /// History records by message type (`user`, `assistant`, `tool_use`, ...).
  pub messages: BTreeMap<String, u64>,
}

/// Open once per database and clone the handle. One thread owns SQLite and the ordinary LRU.
#[derive(Clone)]
pub struct Storage {
  worker: Arc<Worker>,
}
impl Storage {
  pub fn open(path: impl AsRef<Path>, options: StorageOptions) -> Result<Self, StorageError> {
    Self::start_worker(Source::File(path.as_ref().to_owned()), options)
  }
  pub fn open_in_memory(options: StorageOptions) -> Result<Self, StorageError> {
    Self::start_worker(Source::Memory, options)
  }
  fn start_worker(source: Source, options: StorageOptions) -> Result<Self, StorageError> {
    let (sender, receiver) = mpsc::channel::<Command>();
    let (ready, result) = mpsc::sync_channel(1);
    let thread = thread::Builder::new()
      .name("wish-storage".into())
      .spawn(move || {
        let mut database = match Database::open(source, options) {
          Ok(database) => database,
          Err(error) => {
            let _ = ready.send(Err(error));
            return;
          }
        };
        if ready.send(Ok(thread::current().id())).is_err() {
          return;
        }
        while let Ok(command) = receiver.recv() {
          match command {
            Command::Run(job) => job(&mut database),
            Command::Shutdown(reply) => {
              let result = database.flush().and_then(|_| {
                database.connection.close().map_err(|(_, error)| StorageError::from(error))
              });
              let _ = reply.send(result);
              break;
            }
          }
        }
      })
      .map_err(StorageError::StartWorker)?;
    match result.recv().map_err(|_| StorageError::Closed)? {
      Ok(id) => {
        Ok(Self { worker: Arc::new(Worker { sender: Some(sender), thread: Some(thread), id }) })
      }
      Err(error) => {
        let _ = thread.join();
        Err(error)
      }
    }
  }
  fn dispatch<R: Send + 'static>(
    &self,
    operation: impl FnOnce(&mut Database) -> R + Send + 'static,
  ) -> Result<R, StorageError> {
    if thread::current().id() == self.worker.id {
      return Err(StorageError::NestedOperation);
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    self
      .worker
      .sender
      .as_ref()
      .ok_or(StorageError::Closed)?
      .send(Command::Run(Box::new(move |database| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(database)))
          .map_err(|_| StorageError::OperationPanicked);
        let _ = sender.send(result);
      })))
      .map_err(|_| StorageError::Closed)?;
    receiver.recv().map_err(|_| StorageError::Closed)?
  }
  /// Runs on the storage thread. Capture owned data with `move`; do not call handles from inside
  /// this closure. Use Transaction methods to keep all related writes atomic.
  pub fn transaction<R, E>(
    &self,
    edit: impl FnOnce(&mut Transaction<'_>) -> Result<R, E> + Send + 'static,
  ) -> Result<R, E>
  where
    R: Send + 'static,
    E: From<StorageError> + Send + 'static,
  {
    self.run_transaction(edit, true)
  }
  /// Runs `edit` like [`Storage::transaction`], then rolls it back: what it returns tells what
  /// the edit would do, and nothing changes.
  pub fn rehearse<R, E>(
    &self,
    edit: impl FnOnce(&mut Transaction<'_>) -> Result<R, E> + Send + 'static,
  ) -> Result<R, E>
  where
    R: Send + 'static,
    E: From<StorageError> + Send + 'static,
  {
    self.run_transaction(edit, false)
  }
  fn run_transaction<R, E>(
    &self,
    edit: impl FnOnce(&mut Transaction<'_>) -> Result<R, E> + Send + 'static,
    commit: bool,
  ) -> Result<R, E>
  where
    R: Send + 'static,
    E: From<StorageError> + Send + 'static,
  {
    self
      .dispatch(move |database| {
        let sql = database.connection.transaction().map_err(StorageError::from).map_err(E::from)?;
        let mut tx = Transaction { sql, cache: &mut database.cache, dirty: HashSet::new() };
        let result = edit(&mut tx)?;
        let Transaction { sql, cache, dirty } = tx;
        if commit {
          sql.commit().map_err(StorageError::from).map_err(E::from)?;
          for key in dirty {
            cache.invalidate(&key);
          }
        } else {
          // The cache never took a value the edit wrote, so there is nothing to evict.
          sql.rollback().map_err(StorageError::from).map_err(E::from)?;
        }
        Ok(result)
      })
      .map_err(E::from)?
  }
  /// Rebuild the database file without the pages deleted and released data left free, so it
  /// shrinks on disk; the search index is merged first, so the text of deleted history leaves it
  /// too. The rebuilt file is in incremental auto-vacuum mode, which [`Storage::shrink`] needs.
  /// Other storage work waits until it finishes.
  pub fn vacuum(&self) -> Result<(), StorageError> {
    self.dispatch(|database| -> Result<(), StorageError> {
      super::history_index::compact_index(&database.connection)?;
      database.connection.execute_batch("VACUUM")?;
      database.connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
      Ok(())
    })?
  }
  /// Give the room deleted data took back to the file system without rebuilding the file: the
  /// search index is merged as in [`Storage::vacuum`] and the free pages are cut off the end. Its
  /// cost follows what was deleted and the index, not the whole database, so it suits deleting a
  /// session. A database not yet in incremental auto-vacuum mode keeps its free pages for reuse.
  pub fn shrink(&self) -> Result<(), StorageError> {
    self.dispatch(|database| -> Result<(), StorageError> {
      super::history_index::compact_index(&database.connection)?;
      // Each step frees one page and returns a row.
      let mut freeing = database.connection.prepare("PRAGMA incremental_vacuum")?;
      let mut pages = freeing.query([])?;
      while pages.next()?.is_some() {}
      database.connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
      Ok(())
    })?
  }
  /// A read-only handle on the list `id`; opening it reads nothing.
  pub fn open_list<T: StoredValue>(&self, id: &ListId) -> ReadList<T> {
    ReadList::new(self.clone(), id.clone())
  }
  /// What each namespace directly below `parent` holds, by the child's name (`parent/<child>`):
  /// the bytes of its objects, its lists' items and their history index, and how many history
  /// records it has of each message type. Bytes are the stored values, not pages on disk, so the
  /// search index built over the history and SQLite's own overhead are not included.
  pub fn measure_children(
    &self,
    parent: &str,
  ) -> Result<BTreeMap<String, NamespaceUsage>, StorageError> {
    let prefix = format!("{parent}/");
    self.dispatch(move |database| -> Result<_, StorageError> {
      let sql = &database.connection;
      let child = "substr(name, length(?1) + 1, instr(substr(name, length(?1) + 1) || '/', '/') - 1)";
      let mut usage: BTreeMap<String, NamespaceUsage> = BTreeMap::new();
      for query in [
        format!("SELECT {child}, sum(length(value)) FROM wish_objects WHERE substr(name, 1, length(?1)) = ?1 GROUP BY 1"),
        format!("SELECT {child}, sum(length(item.value)) FROM wish_items item JOIN wish_list_keys list ON list.id = item.list WHERE substr(name, 1, length(?1)) = ?1 GROUP BY 1"),
      ] {
        let mut statement = sql.prepare(&query)?;
        let rows = statement.query_map([&prefix], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)))?;
        for row in rows {
          let (name, bytes) = row?;
          usage.entry(name).or_default().bytes += bytes.unwrap_or(0).max(0) as u64;
        }
      }
      super::history_index::measure_children(sql, &prefix, child, &mut usage)?;
      Ok(usage)
    })?
  }
  /// Checkpoint the WAL in full and close this storage worker for every cloned handle. Await all
  /// runs/producers first. Commands queued before shutdown finish; later commands fail with Closed.
  /// Errors, `CheckpointBusy` among them, are reported even though the worker still closes.
  pub fn shutdown(&self) -> Result<(), StorageError> {
    if thread::current().id() == self.worker.id {
      return Err(StorageError::NestedOperation);
    }
    let (reply, result) = mpsc::sync_channel(1);
    self
      .worker
      .sender
      .as_ref()
      .ok_or(StorageError::Closed)?
      .send(Command::Shutdown(reply))
      .map_err(|_| StorageError::Closed)?;
    result.recv().map_err(|_| StorageError::Closed)?
  }

  pub(crate) fn claim_owner(&self, name: &str) -> Result<OwnerGuard, StorageError> {
    let name = name.to_owned();
    let target = name.clone();
    self.dispatch(move |database| {
      if database.owners.insert(target.clone()) {
        Ok(())
      } else {
        Err(StorageError::AlreadyOpen(target))
      }
    })??;
    Ok(OwnerGuard { storage: self.clone(), name })
  }
}
/// A local ownership token, not a database lock or a cross-process lease.
pub(crate) struct OwnerGuard {
  storage: Storage,
  name: String,
}
impl Drop for OwnerGuard {
  fn drop(&mut self) {
    let name = self.name.clone();
    if let Some(sender) = &self.storage.worker.sender {
      let _ = sender.send(Command::Run(Box::new(move |database| {
        database.owners.remove(&name);
      })));
    }
  }
}
impl Database {
  /// Synchronize committed WAL data into the database file.
  fn flush(&mut self) -> Result<(), StorageError> {
    let busy: i64 =
      self.connection.query_row("PRAGMA wal_checkpoint(FULL)", [], |row| row.get(0))?;
    if busy != 0 {
      return Err(StorageError::CheckpointBusy);
    }
    Ok(())
  }

  fn open(source: Source, options: StorageOptions) -> Result<Self, StorageError> {
    let mut connection = match source {
      Source::File(path) => Connection::open(path)?,
      Source::Memory => Connection::open_in_memory()?,
    };
    // Incremental auto-vacuum lets `shrink` return free pages. It takes effect when the database
    // is created, and for an older file at its next `vacuum`.
    connection.execute_batch(
      "PRAGMA auto_vacuum=INCREMENTAL; PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;
      PRAGMA synchronous=NORMAL;",
    )?;
    // A new database starts at 0. Any other version is refused: older ones are brought up to date
    // before the program opens storage at all.
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !matches!(version, 0 | SCHEMA_VERSION) {
      return Err(StorageError::SchemaVersion(version));
    }
    let tx = connection.transaction()?;
    tx.execute_batch(
      "CREATE TABLE IF NOT EXISTS wish_list_keys (
      id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
      CREATE TABLE IF NOT EXISTS wish_objects (
      name TEXT PRIMARY KEY,kind TEXT NOT NULL,value BLOB NOT NULL);
      CREATE TABLE IF NOT EXISTS wish_lists (
      name TEXT PRIMARY KEY,kind TEXT NOT NULL,length INTEGER NOT NULL CHECK(length>=0));
      CREATE TABLE IF NOT EXISTS wish_items (
      list INTEGER NOT NULL REFERENCES wish_list_keys(id),position INTEGER NOT NULL CHECK(position>=0),
      value BLOB NOT NULL,PRIMARY KEY(list,position)) WITHOUT ROWID;",
    )?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    super::history_index::create_schema(&tx)?;
    tx.commit()?;
    Ok(Self { connection, cache: Cache::new(options.cache_capacity_bytes), owners: HashSet::new() })
  }
}
