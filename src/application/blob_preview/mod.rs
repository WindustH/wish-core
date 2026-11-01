//! Blob decoding and preview generation: bounded shared cache and worker budget.
pub mod image;
pub mod pdf;
pub(crate) mod worker;
use super::error::{ApplicationError, ErrorKind};
use std::{collections::VecDeque, process::Stdio, sync::LazyLock};
use tokio::{
  io::AsyncReadExt,
  sync::{Mutex, Semaphore},
};
const MAX_CACHE_BYTES: usize = 128 * 1024 * 1024;
type Entries = VecDeque<(String, bytes::Bytes)>;
static CACHE: LazyLock<Mutex<Entries>> = LazyLock::new(|| Mutex::new(VecDeque::new()));
static WORKERS: Semaphore = Semaphore::const_new(2);
pub(super) async fn cached(key: &str) -> Option<bytes::Bytes> {
  let mut cache = CACHE.lock().await;
  let position = cache.iter().position(|(id, _)| id == key)?;
  let entry = cache.remove(position)?;
  let data = entry.1.clone();
  cache.push_back(entry);
  Some(data)
}

pub struct Source {
  pub etag: String,
  key: String,
  file: std::path::PathBuf,
}
pub struct Preview {
  pub bytes: bytes::Bytes,
}
pub(super) async fn remember(key: String, bytes: Vec<u8>) -> bytes::Bytes {
  let bytes = bytes::Bytes::from(bytes);
  let mut cache = CACHE.lock().await;
  cache.retain(|(existing, _)| existing != &key);
  cache.push_back((key, bytes.clone()));
  while cache.len() > 128
    || cache.iter().map(|(_, bytes)| bytes.len()).sum::<usize>() > MAX_CACHE_BYTES
  {
    cache.pop_front();
  }
  bytes
}
pub(super) async fn execute(
  mut command: tokio::process::Command,
  limit: u64,
) -> Result<Vec<u8>, ApplicationError> {
  command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
  let mut child = command.spawn().map_err(ApplicationError::internal)?;
  tokio::time::timeout(std::time::Duration::from_secs(30), async {
    let mut bytes = Vec::new();
    child
      .stdout
      .take()
      .expect("piped output")
      .take(limit + 1)
      .read_to_end(&mut bytes)
      .await
      .map_err(ApplicationError::internal)?;
    if bytes.len() as u64 > limit {
      return Err(ApplicationError::invalid("Blob preview exceeds output limit"));
    }
    if !child.wait().await.map_err(ApplicationError::internal)?.success() || bytes.is_empty() {
      return Err(ApplicationError::invalid("Unable to decode this file for preview"));
    }
    Ok(bytes)
  })
  .await
  .map_err(|_| ApplicationError {
    kind: ErrorKind::Timeout,
    message: "Blob preview timed out".into(),
    details: None,
  })?
}

pub(super) fn command(
  kind: &str,
  file: &std::path::Path,
) -> Result<tokio::process::Command, ApplicationError> {
  let mut command =
    tokio::process::Command::new(std::env::current_exe().map_err(ApplicationError::internal)?);
  command.arg("--blob-worker").arg(kind).arg(file);
  Ok(command)
}
