//! Lifecycle and quotas for attachments uploaded before a session exists.
use super::{
  app::App,
  error::{ApplicationError, blocking},
};
use crate::storage::blobs;
use std::{sync::Arc, time::Duration};
const MAX_DRAFTS: usize = 128;
const MAX_BYTES: u64 = 512 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
async fn inspect(
  app: &Arc<App>,
  incoming: Option<String>,
  guard: tokio::sync::OwnedMutexGuard<()>,
) -> Result<(usize, u64, tokio::sync::OwnedMutexGuard<()>), ApplicationError> {
  let root = app.data_dir.blob_root();
  let management = app.management.clone();
  blocking(move || {
    let mut count = 0;
    let mut bytes: u64 = 0;
    let entries = match std::fs::read_dir(root) {
      Ok(entries) => entries,
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0, guard)),
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
      if incoming.as_deref() != Some(&id) {
        count += 1;
      }
      for file in std::fs::read_dir(entry.path()).map_err(ApplicationError::internal)? {
        let file = file.map_err(ApplicationError::internal)?;
        if file.file_type().map_err(ApplicationError::internal)?.is_file() {
          bytes = bytes.saturating_add(file.metadata().map_err(ApplicationError::internal)?.len());
        }
      }
    }
    Ok((count, bytes, guard))
  })
  .await
}
pub(super) async fn upload(
  app: &Arc<App>,
  id: String,
  body: &[u8],
) -> Result<blobs::Blob, ApplicationError> {
  let creation = app.creation_lock.clone().lock_owned().await;
  let (count, bytes, _creation) = inspect(app, Some(id.clone()), creation).await?;
  if count >= MAX_DRAFTS || bytes.saturating_add(body.len() as u64) > MAX_BYTES {
    return Err(ApplicationError::conflict(
      "pending attachment limit reached; send or remove pending drafts",
    ));
  }
  Ok(blobs::save_upload(&app.data_dir.blobs(&id), body).await?)
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
          let creation = app.creation_lock.clone().lock_owned().await;
          if let Err(error) = inspect(&app, None, creation).await { eprintln!("cleaning unused attachment drafts: {error}"); }
        }
      }
    }
  });
}
