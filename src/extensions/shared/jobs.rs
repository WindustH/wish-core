//! Schema-free durable job infrastructure. Extensions own states, scope and recovery policy.
//! A lock lives only while a caller holds or waits for it; reads and writes are bounded.
use serde::{Serialize, de::DeserializeOwned};
use std::{
  collections::BTreeMap,
  path::{Path, PathBuf},
  sync::{Arc, Mutex, Weak},
};
use tokio::io::AsyncReadExt;

const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct JobLocks {
  entries: Mutex<BTreeMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>,
}
impl JobLocks {
  pub(crate) async fn lock(&self, path: &Path) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
      let mut entries = self.entries.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
      entries.retain(|_, lock| lock.strong_count() > 0);
      if let Some(lock) = entries.get(path).and_then(Weak::upgrade) {
        lock
      } else {
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        entries.insert(path.to_owned(), Arc::downgrade(&lock));
        lock
      }
    };
    lock.lock_owned().await
  }
}

pub(crate) async fn read<T: DeserializeOwned + Send + 'static>(path: &Path) -> Result<T, String> {
  let file = tokio::fs::File::open(path).await.map_err(|error| {
    if error.kind() == std::io::ErrorKind::NotFound {
      "job record not found".to_owned()
    } else {
      format!("cannot open job record: {error}")
    }
  })?;
  if file.metadata().await.map_err(|e| e.to_string())?.len() > MAX_RECORD_BYTES as u64 {
    return Err("job record exceeds 2 MiB limit".into());
  }
  let mut bytes = Vec::new();
  file
    .take(MAX_RECORD_BYTES as u64 + 1)
    .read_to_end(&mut bytes)
    .await
    .map_err(|e| e.to_string())?;
  if bytes.len() > MAX_RECORD_BYTES {
    return Err("job record exceeds 2 MiB limit".into());
  }
  crate::utils::blocking::cpu(move || {
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid job record: {e}"))
  })
  .await
  .map_err(|e| e.to_string())?
}

pub(crate) async fn write<T: Serialize + Send + 'static>(
  path: &Path,
  record: T,
) -> Result<(), String> {
  // Serialize on the CPU pool with a bounded writer, never allocate an unbounded JSON buffer.
  let bytes = crate::utils::blocking::cpu(move || {
    struct Bounded(Vec<u8>);
    impl std::io::Write for Bounded {
      fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_RECORD_BYTES.saturating_sub(self.0.len()) {
          return Err(std::io::Error::other("job record exceeds 2 MiB limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
      }
      fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
      }
    }
    let mut writer = Bounded(Vec::new());
    serde_json::to_writer(&mut writer, &record).map_err(|e| e.to_string())?;
    Ok::<_, String>(writer.0)
  })
  .await
  .map_err(|e| e.to_string())??;
  crate::storage::blobs::atomic_write(path, &bytes).await.map_err(|e| e.to_string())
}
