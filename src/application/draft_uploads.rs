//! Lifecycle and quotas for attachments uploaded before a session exists.
use super::{
  app::App,
  error::{ApplicationError, blocking},
};
use crate::storage::blobs;
use std::{collections::BTreeMap, sync::Arc, time::Duration};
const MAX_DRAFTS: usize = 128;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Exact pending-directory sizes. Built once, updated on upload/adoption, and reconciled by
/// the hourly reaper. Never shares the session creation lock.
#[derive(Default)]
pub(super) struct DraftInventory {
  sizes: BTreeMap<String, u64>,
  loaded: bool,
}
impl DraftInventory {
  pub(super) fn adopted(&mut self, id: &str) {
    self.sizes.remove(id);
  }
}
async fn inspect(
  app: &Arc<App>,
  incoming: Option<String>,
  mut guard: tokio::sync::OwnedMutexGuard<DraftInventory>,
) -> Result<tokio::sync::OwnedMutexGuard<DraftInventory>, ApplicationError> {
  let root = app.data_dir.blob_root();
  let management = app.management.clone();
  blocking(move || {
    guard.loaded = false;
    guard.sizes.clear();
    let entries = match std::fs::read_dir(root) {
      Ok(entries) => entries,
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
        guard.loaded = true;
        return Ok(guard);
      }
      Err(e) => return Err(ApplicationError::internal(e)),
    };
    for entry in entries {
      let entry = entry.map_err(ApplicationError::internal)?;
      if !entry.file_type().map_err(ApplicationError::internal)?.is_dir() {
        continue;
      }
      let id = entry.file_name().to_string_lossy().into_owned();
      if uuid::Uuid::parse_str(&id).is_err()
        || management.exists(&id)?
        || management.group(&id)?.is_some()
      {
        continue;
      }
      let modified =
        entry.metadata().and_then(|m| m.modified()).map_err(ApplicationError::internal)?;
      if modified.elapsed().is_ok_and(|age| age >= TTL) && incoming.as_deref() != Some(&id) {
        std::fs::remove_dir_all(entry.path()).map_err(ApplicationError::internal)?;
        continue;
      }
      let mut bytes = 0u64;
      for file in std::fs::read_dir(entry.path()).map_err(ApplicationError::internal)? {
        let file = file.map_err(ApplicationError::internal)?;
        if file.file_type().map_err(ApplicationError::internal)?.is_file() {
          bytes = bytes.saturating_add(file.metadata().map_err(ApplicationError::internal)?.len());
        }
      }
      guard.sizes.insert(id, bytes);
    }
    guard.loaded = true;
    Ok(guard)
  })
  .await
}
pub(super) async fn upload(
  app: &Arc<App>,
  id: String,
  body: &[u8],
) -> Result<blobs::Blob, ApplicationError> {
  let mut inventory = app.drafts.clone().lock_owned().await;
  if !inventory.loaded {
    inventory = inspect(app, Some(id.clone()), inventory).await?;
  }
  // A retry can upload under an already accepted UUID. It is an ordinary session blob,
  // not another pending draft, and must not poison pending quota accounting.
  let management = app.management.clone();
  let owner = id.clone();
  let accepted =
    blocking(move || Ok(management.exists(&owner)? || management.group(&owner)?.is_some())).await?;
  if accepted {
    return Ok(blobs::save_upload(&app.data_dir.blobs(&id), body).await?);
  }
  let count = inventory.sizes.len() - usize::from(inventory.sizes.contains_key(&id));
  let bytes = inventory.sizes.values().copied().fold(0u64, u64::saturating_add);
  if count >= MAX_DRAFTS || bytes.saturating_add(body.len() as u64) > MAX_BYTES {
    return Err(ApplicationError::conflict(
      "pending attachment limit reached; send or remove pending drafts",
    ));
  }
  // If a cancelled/failed write leaves complete bytes behind, reconcile before admitting
  // another upload. Cancellation drops this guard but cannot leave an optimistic counter.
  inventory.loaded = false;
  let directory = app.data_dir.blobs(&id);
  let blob = blobs::save_upload(&directory, body).await?;
  let size = blocking(move || {
    let mut size = 0u64;
    for file in std::fs::read_dir(directory).map_err(ApplicationError::internal)? {
      let file = file.map_err(ApplicationError::internal)?;
      if file.file_type().map_err(ApplicationError::internal)?.is_file() {
        size = size.saturating_add(file.metadata().map_err(ApplicationError::internal)?.len());
      }
    }
    Ok(size)
  })
  .await?;
  inventory.sizes.insert(id, size);
  inventory.loaded = true;
  Ok(blob)
}

pub(super) fn start_reaper(app: &Arc<App>) {
  let weak = Arc::downgrade(app);
  let stop = app.lifecycle.stop.clone();
  app.lifecycle.tasks.spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(3600));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
      tokio::select! {
        _ = stop.cancelled() => break,
        _ = interval.tick() => {
          let Some(app) = weak.upgrade() else { break; };
          let drafts = app.drafts.clone().lock_owned().await;
          if let Err(error) = inspect(&app, None, drafts).await { eprintln!("cleaning unused attachment drafts: {error}"); }
        }
      }
    }
  });
}
