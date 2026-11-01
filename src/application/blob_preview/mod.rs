//! Blob decoding and preview generation: bounded shared cache and worker budget.
pub mod image;
pub mod pdf;
use super::error::{ApplicationError, ErrorKind};
use std::{
  collections::VecDeque,
  process::Stdio,
  sync::{Arc, LazyLock},
};
use tokio::{
  io::AsyncReadExt,
  sync::{Mutex, Semaphore},
};
const LIMIT: usize = 128 * 1024 * 1024;
type Entries = VecDeque<(String, Arc<Vec<u8>>)>;
static CACHE: LazyLock<Mutex<Entries>> = LazyLock::new(|| Mutex::new(VecDeque::new()));
static WORKERS: Semaphore = Semaphore::const_new(2);
pub(super) async fn cached(key: &str) -> Option<Arc<Vec<u8>>> {
  let mut cache = CACHE.lock().await;
  let position = cache.iter().position(|(id, _)| id == key)?;
  let entry = cache.remove(position)?;
  let data = entry.1.clone();
  cache.push_back(entry);
  Some(data)
}

pub struct Preview {
  pub bytes: Arc<Vec<u8>>,
  pub etag: String,
}
pub(super) async fn remember(key: String, bytes: Vec<u8>) -> Arc<Vec<u8>> {
  let bytes = Arc::new(bytes);
  let mut cache = CACHE.lock().await;
  cache.push_back((key, bytes.clone()));
  while cache.len() > 128 || cache.iter().map(|(_, bytes)| bytes.len()).sum::<usize>() > LIMIT {
    cache.pop_front();
  }
  bytes
}
pub(super) async fn execute(
  mut command: tokio::process::Command,
  tool: &str,
  limit: u64,
) -> Result<Vec<u8>, ApplicationError> {
  command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
  let mut child = command.spawn().map_err(|error| {
    if error.kind() == std::io::ErrorKind::NotFound {
      ApplicationError {
        kind: ErrorKind::Unsupported,
        message: format!("Blob previews require {tool} on the Wish server"),
        details: None,
      }
    } else {
      ApplicationError::internal(error)
    }
  })?;
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
