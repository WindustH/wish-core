//! Lifecycle and quotas for attachments uploaded before a session exists.
use super::{
  app::App,
  error::{ApplicationError, blocking},
};
use crate::storage::blobs;
use std::{
  collections::BTreeMap,
  sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};
const MAX_DRAFTS: usize = 128;
const MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Exact pending-directory sizes. Built once, updated on upload/adoption, and reconciled by
/// the hourly reaper. Never shares the session creation lock.
#[derive(Default)]
pub(super) struct DraftInventory {
  sizes: BTreeMap<String, u64>,
  loaded: bool,
  dirty: Arc<AtomicBool>,
  reservations: BTreeMap<String, (u64, Arc<AtomicBool>)>,
  owners: BTreeMap<String, Weak<tokio::sync::Mutex<()>>>,
}
impl DraftInventory {
  pub(super) fn adopted(&mut self, id: &str) {
    self.sizes.remove(id);
  }
}
// Only uploads/adoption for the same UUID wait for its network stream.
pub(super) async fn lock_owner(app: &Arc<App>, id: &str) -> tokio::sync::OwnedMutexGuard<()> {
  let lock = {
    let mut inventory = app.drafts.lock().await;
    inventory.owners.retain(|_, value| value.strong_count() > 0);
    let lock = inventory
      .owners
      .get(id)
      .and_then(Weak::upgrade)
      .unwrap_or_else(|| Arc::new(tokio::sync::Mutex::new(())));
    inventory.owners.insert(id.to_owned(), Arc::downgrade(&lock));
    lock
  };
  lock.lock_owned().await
}
struct Reservation {
  reconciled: bool,
  active: Arc<AtomicBool>,
  dirty: Arc<AtomicBool>,
}
impl Drop for Reservation {
  fn drop(&mut self) {
    // Synchronous cancellation cleanup: the next admission reconciles completed files,
    // including cancellation after atomic publication but before the HTTP response.
    if !self.reconciled {
      self.dirty.store(true, Ordering::SeqCst);
    }
    self.active.store(false, Ordering::SeqCst);
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
    guard.dirty.store(false, Ordering::SeqCst);
    guard.reservations.retain(|_, (_, active)| active.load(Ordering::SeqCst));
    let active = guard.reservations.keys().cloned().collect::<std::collections::BTreeSet<_>>();
    guard.sizes.retain(|id, _| active.contains(id));
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
      if active.contains(&id) {
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
pub(super) async fn upload_stream<S, B, E>(
  app: &Arc<App>,
  id: String,
  stream: S,
) -> Result<blobs::Blob, ApplicationError>
where
  S: futures_util::Stream<Item = Result<B, E>>,
  B: AsRef<[u8]>,
  E: std::fmt::Display,
{
  let _owner = lock_owner(app, &id).await;
  let mut inventory = app.drafts.clone().lock_owned().await;
  if !inventory.loaded || inventory.dirty.load(Ordering::SeqCst) {
    inventory = inspect(app, Some(id.clone()), inventory).await?;
  }
  // A retry can upload under an already accepted UUID. It is an ordinary session blob,
  // not another pending draft, and must not poison pending quota accounting.
  let management = app.management.clone();
  let owner = id.clone();
  let accepted =
    blocking(move || Ok(management.exists(&owner)? || management.group(&owner)?.is_some())).await?;
  if accepted {
    drop(inventory);
    return Ok(
      blobs::save_upload_stream(&app.data_dir.blobs(&id), stream, blobs::MAX_UPLOAD_BYTES).await?,
    );
  }
  let count = inventory.sizes.len() - usize::from(inventory.sizes.contains_key(&id));
  let bytes = inventory.sizes.values().copied().fold(0u64, u64::saturating_add).saturating_add(
    inventory
      .reservations
      .values()
      .filter(|(_, active)| active.load(Ordering::SeqCst))
      .map(|(bytes, _)| *bytes)
      .sum::<u64>(),
  );
  if count >= MAX_DRAFTS || bytes >= MAX_BYTES {
    return Err(ApplicationError::conflict(
      "pending attachment limit reached; send or remove pending drafts",
    ));
  }
  // The sidecar is bounded by the descriptor reader; reserve its space as well.
  const DESCRIPTOR_BYTES: u64 = 64 * 1024;
  if MAX_BYTES - bytes <= DESCRIPTOR_BYTES {
    return Err(ApplicationError::conflict("pending attachment limit reached"));
  }
  let allowance = (MAX_BYTES - bytes - DESCRIPTOR_BYTES).min(blobs::MAX_UPLOAD_BYTES as u64);
  let active = Arc::new(AtomicBool::new(true));
  inventory.sizes.entry(id.clone()).or_default();
  inventory.reservations.insert(id.clone(), (allowance + DESCRIPTOR_BYTES, active.clone()));
  let mut reservation = Reservation { reconciled: false, active, dirty: inventory.dirty.clone() };
  drop(inventory);
  let directory = app.data_dir.blobs(&id);
  let blob = blobs::save_upload_stream(&directory, stream, allowance as usize).await?;
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
  let mut inventory = app.drafts.lock().await;
  inventory.sizes.insert(id.clone(), size);
  inventory.reservations.remove(&id);
  reservation.reconciled = true;
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
