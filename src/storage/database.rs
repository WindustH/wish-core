use super::cache::Cache;
use super::{ListId, StorageError, StoredList, StoredObject, StoredValue, Transaction};
use rusqlite::Connection;
use std::{
  collections::HashSet,
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
    self
      .dispatch(move |database| {
        let sql = database.connection.transaction().map_err(StorageError::from).map_err(E::from)?;
        let mut tx = Transaction { sql, cache: &mut database.cache, dirty: HashSet::new() };
        let result = edit(&mut tx)?;
        let Transaction { sql, cache, dirty } = tx;
        sql.commit().map_err(StorageError::from).map_err(E::from)?;
        for key in dirty {
          cache.invalidate(&key);
        }
        Ok(result)
      })
      .map_err(E::from)?
  }
  pub fn open_list<T: StoredValue>(&self, id: &ListId) -> StoredList<T> {
    StoredList::new(self.clone(), id.clone())
  }
  pub fn open_object<T: StoredValue>(&self, name: impl Into<String>) -> StoredObject<T> {
    StoredObject::new(self.clone(), name.into())
  }
  pub fn create_list<T: StoredValue>(
    &self,
    name: impl Into<String>,
  ) -> Result<StoredList<T>, StorageError> {
    let id = ListId(name.into());
    let target = id.clone();
    self.transaction(move |tx| tx.create_list::<T>(&target))?;
    Ok(self.open_list(&id))
  }
  pub fn clear_cache(&self) -> Result<(), StorageError> {
    self.dispatch(|database| database.cache.clear())
  }

  /// Wait for earlier storage commands and synchronize committed WAL data.
  /// Call after awaiting agent runs; live model stream events are never stored.
  pub fn flush(&self) -> Result<(), StorageError> {
    self.dispatch(Database::flush)?
  }

  /// Close this storage worker for every cloned handle. Await all runs/producers first.
  /// Commands queued before shutdown finish; later commands fail with Closed.
  /// Errors are reported even though the worker still closes.
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
    connection.execute_batch(
      "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;",
    )?;
    // A new database starts at 0. Any other version is migrated by hand before this build opens it.
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if !matches!(version, 0 | 4) {
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
      value BLOB NOT NULL,PRIMARY KEY(list,position)) WITHOUT ROWID;
      PRAGMA user_version=4;",
    )?;
    super::search::create_schema(&tx)?;
    tx.commit()?;
    Ok(Self { connection, cache: Cache::new(options.cache_capacity_bytes), owners: HashSet::new() })
  }
}
