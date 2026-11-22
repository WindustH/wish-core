//! Verified packages for temporary exports and durable cleanup snapshots.
pub(super) mod session;
mod writer;
use super::{
  app::App,
  error::{ApplicationError as Error, blocking},
};
use crate::storage::management::archives::{Filter, PendingRelease};
use serde_json::{Value, json};
pub use session::Include;
use std::{path::Path, sync::Arc};
pub(super) use writer::Writer;

pub(super) fn package_lock(id: &str) -> Arc<std::sync::Mutex<()>> {
  static LOCKS: std::sync::LazyLock<crate::utils::keyed_lock::KeyedLocks<String>> =
    std::sync::LazyLock::new(Default::default);
  LOCKS.get(id.to_owned())
}
static PERMITS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
pub(super) fn permit() -> Result<tokio::sync::SemaphorePermit<'static>, Error> {
  PERMITS.try_acquire().map_err(|_| Error::conflict("two packages are already being prepared"))
}
pub(super) fn package_id(manifest: &Value) -> Result<&str, Error> {
  manifest["archive_id"]
    .as_str()
    .filter(|id| uuid::Uuid::parse_str(id).is_ok())
    .ok_or_else(|| Error::internal("invalid package ID"))
}
pub(super) fn persist(
  app: &App,
  manifest: &Value,
  release: Option<&PendingRelease>,
) -> Result<(), Error> {
  let export = manifest["expires_at"].is_number();
  match app.management.record_archive(manifest, release) {
    Ok(evicted) => {
      for id in evicted {
        let _ = std::fs::remove_file(app.data_dir.packages(true).join(format!("{id}.zip")));
      }
    }
    Err(e) => {
      let id = package_id(manifest)?;
      let _ = std::fs::remove_file(app.data_dir.packages(export).join(format!("{id}.zip")));
      return Err(e.into());
    }
  }
  Ok(())
}
pub(super) fn discard_unapplied(app: &App, manifest: &Value) -> Result<(), Error> {
  let id = package_id(manifest)?;
  app.management.remove_package(id, false)?;
  std::fs::remove_file(app.data_dir.packages(false).join(format!("{id}.zip")))
    .map_err(Error::internal)
}
pub async fn package(
  app: &Arc<App>,
  id: String,
  export: bool,
  options: Include,
) -> Result<Value, Error> {
  let owner = app.clone();
  app
    .lifecycle
    .while_open(|| {
      app.lifecycle.tasks.spawn(async move {
        let _permit = permit()?;
        super::session_edit::release(&owner, &id, Some((export, options))).await
      })
    })
    .ok_or_else(Error::shutting_down)?
    .await
    .map_err(Error::internal)?
}
pub async fn list(app: &Arc<App>, query: Filter) -> Result<Value, Error> {
  let db = app.management.clone();
  blocking(move || Ok(db.list_archives(query)?)).await
}
pub async fn detail(app: &Arc<App>, id: &str, export: bool) -> Result<Value, Error> {
  let db = app.management.clone();
  let id = id.to_owned();
  blocking(move || Ok(db.package(&id, export, crate::utils::time::Timestamp::now().0)?)).await
}
pub fn filename(m: &Value) -> Result<String, Error> {
  let kind = m["kind"].as_str().ok_or_else(|| Error::internal("missing archive kind"))?;
  let field = if m["disposition"] == "export" { "session_id" } else { "archive_id" };
  let id = m[field]
    .as_str()
    .and_then(|id| uuid::Uuid::parse_str(id).ok())
    .ok_or_else(|| Error::internal("invalid package identity"))?;
  Ok(if field == "session_id" {
    format!("session-{id}.zip")
  } else {
    format!("archive-{kind}-{id}.zip")
  })
}
pub async fn download(
  app: &Arc<App>,
  id: &str,
  export: bool,
) -> Result<(super::attachments::BlobReader, String), Error> {
  let m = detail(app, id, export).await?;
  let path = app.data_dir.packages(export).join(format!("{id}.zip"));
  let file = tokio::fs::File::open(&path).await.map_err(|e| {
    if e.kind() == std::io::ErrorKind::NotFound { Error::not_found() } else { Error::internal(e) }
  })?;
  let name = filename(&m)?;
  Ok((
    super::attachments::BlobReader {
      length: file.metadata().await.map_err(Error::internal)?.len(),
      file,
      path,
      mime_type: "application/zip".into(),
      version: format!("\"{}\"", m["sha256"].as_str().unwrap_or(id)),
    },
    name,
  ))
}
pub async fn remove(app: &Arc<App>, id: String, export: bool) -> Result<(), Error> {
  let db = app.management.clone();
  let dir = app.data_dir.packages(export);
  blocking(move || {
    let lock = package_lock(&id);
    let _guard = lock.lock().map_err(Error::internal)?;
    match db.require_removable(&id, export) {
      Err(crate::storage::repository_error::RepositoryError::Missing) if export => return Ok(()),
      result => {
        result?;
      }
    }
    let source = dir.join(format!("{id}.zip"));
    let removing = dir.join(format!("{id}.removing"));
    let moved = match std::fs::rename(&source, &removing) {
      Ok(()) => true,
      Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
      Err(e) => return Err(Error::internal(e)),
    };
    if let Err(error) = db.remove_package(&id, export) {
      if moved {
        std::fs::rename(&removing, source).map_err(Error::internal)?;
      }
      return Err(error.into());
    }
    if moved {
      std::fs::remove_file(removing).map_err(Error::internal)?;
    }
    Ok(())
  })
  .await
}
pub async fn expire(app: &Arc<App>, now: u64) -> Result<(), Error> {
  let db = app.management.clone();
  let ids = blocking(move || Ok(db.expired_exports(now)?)).await?;
  for id in ids {
    if let Err(e) = remove(app, id.clone(), true).await {
      eprintln!("expiring export {id}: {e}");
    }
  }
  Ok(())
}
pub async fn recover(app: &Arc<App>) -> Result<(), Error> {
  let db = app.management.clone();
  let (archived, deleted) =
    blocking(move || Ok((db.pending_releases()?, db.pending_deletes()?))).await?;
  let pending = archived
    .into_iter()
    .map(|(aid, owner, journal)| (Some(aid), owner, journal))
    .chain(deleted.into_iter().map(|(id, journal)| (None, id, journal)));
  for (archive, owner, journal) in pending {
    let result: Result<(), Error> = async {
      let pending: PendingRelease = serde_json::from_str(&journal).map_err(Error::internal)?;
      if let Some(path) = &pending.temp_dir {
        super::session_temp::validate(&owner, path)?;
      }
      if let Some(id) = &archive {
        let expected = detail(app, id, false).await?;
        let file = app.data_dir.packages(false).join(format!("{id}.zip"));
        blocking(move || {
          let (hash, size) = writer::digest(&file)?;
          if expected["sha256"] != hash || expected["archive_bytes"] != size {
            return Err(Error::internal("pending archive checksum mismatch"));
          }
          Ok(())
        })
        .await?;
      }
      let storage = app.storage.clone();
      let target = owner.clone();
      blocking(move || Ok(crate::session::Session::delete_closed(&storage, &target)?)).await?;
      super::session_edit::detach_files(app, &owner, pending.temp_dir).await?;
      let db = app.management.clone();
      let target = owner.clone();
      blocking(move || {
        Ok(if let Some(id) = archive {
          db.finish_release(&id)?
        } else {
          db.finish_delete(&target)?
        })
      })
      .await?;
      app.reclaim.notify_one();
      Ok(())
    }
    .await;
    if let Err(e) = result {
      eprintln!("pending session cleanup {owner} retained: {e}");
    }
  }
  let worker = app.clone();
  blocking(move || {
    for export in [false, true] {
      let dir = worker.data_dir.packages(export);
      let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
        Err(e) => return Err(Error::internal(e)),
      };
      for entry in entries {
        let entry = entry.map_err(Error::internal)?;
        if !entry.file_type().map_err(Error::internal)?.is_file() {
          continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(id) =
          name.strip_suffix(".removing").filter(|id| uuid::Uuid::parse_str(id).is_ok())
        {
          match worker.management.package(id, export, 0) {
            Ok(_) => std::fs::rename(entry.path(), entry.path().with_extension("zip"))
              .map_err(Error::internal)?,
            Err(crate::storage::repository_error::RepositoryError::Missing) => {
              std::fs::remove_file(entry.path()).map_err(Error::internal)?
            }
            Err(e) => return Err(e.into()),
          }
        } else if let Some(id) =
          name.strip_suffix(".zip").filter(|id| uuid::Uuid::parse_str(id).is_ok())
        {
          match worker.management.package(id, export, 0) {
            Ok(_) => {}
            Err(crate::storage::repository_error::RepositoryError::Missing) if export => {
              std::fs::remove_file(entry.path()).map_err(Error::internal)?
            }
            Err(crate::storage::repository_error::RepositoryError::Missing) => {
              match writer::read_manifest(&entry.path(), id) {
                Ok(manifest) => {
                  if let Err(e) = worker.management.record_archive(&manifest, None) {
                    eprintln!("retaining unindexed archive {id}: {e}");
                  }
                }
                Err(error) => eprintln!("retaining unreadable unindexed archive {id}: {error}"),
              }
            }
            Err(e) => return Err(e.into()),
          }
        } else if name
          .strip_prefix('.')
          .and_then(|v| v.strip_suffix(".zip.tmp"))
          .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        {
          std::fs::remove_file(entry.path()).map_err(Error::internal)?;
        }
      }
    }
    Ok(())
  })
  .await?;
  expire(app, i64::MAX as u64).await
}
pub(super) fn library_files(w: &mut Writer, dir: &Path, ids: &[String]) -> Result<(), Error> {
  let meta = match std::fs::symlink_metadata(dir.join(".library.json")) {
    Ok(m) if m.is_file() => {
      let bytes =
        crate::utils::bounded_file::read_sync(&dir.join(".library.json"), 16 * 1024 * 1024)
          .map_err(Error::internal)?;
      serde_json::from_slice::<Value>(&bytes).map_err(Error::internal)?
    }
    Ok(_) => return Err(Error::invalid("invalid Library metadata")),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
    Err(e) => return Err(Error::internal(e)),
  };
  let mut subset = json!({"entries":{},"locations":{}});
  for id in ids {
    for field in ["entries", "locations"] {
      if let Some(value) = meta[field].get(id) {
        subset[field][id] = value.clone();
      }
    }
    w.tree(dir, Path::new(id), "blobs")?;
    let description = crate::storage::blobs::description_path(dir, id);
    w.tree(dir, description.strip_prefix(dir).map_err(Error::internal)?, "blobs")?;
  }
  w.json("blobs/.library.json", &subset)
}

pub(super) fn usage(app: &App, path: &Path) -> Result<(i64, i64, Value), Error> {
  let snapshot = crate::storage::management::archives::UsageSnapshot::open(path)?;
  let [calls, samples] = snapshot.counts;
  if calls + samples == 0 {
    return Ok((0, 0, Value::Null));
  }
  let mut writer = Writer::new(app, "usage_prune", None, "Usage records", None, false)?;
  for (samples, name) in [(false, "usage/calls.jsonl"), (true, "usage/stream_samples.jsonl")] {
    writer.entry(name, false)?;
    snapshot.visit(samples, |row| {
      writer.bytes(&serde_json::to_vec(&row).map_err(Error::internal)?)?;
      writer.bytes(b"\n")
    })?;
  }
  writer.manifest["counts"] = json!({"calls":calls,"stream_samples":samples});
  let manifest = writer.finish()?;
  if let Err(error) = snapshot.commit(&app.management, &manifest) {
    let id = package_id(&manifest)?;
    let _ = std::fs::remove_file(app.data_dir.packages(false).join(format!("{id}.zip")));
    return Err(error.into());
  }
  Ok((calls, samples, manifest))
}
