//! Releasing what sessions keep but no longer use: the history their context has moved past, and
//! the attachments, images and command output only that history named.
use crate::{
  application::{
    app::App,
    error::{ApplicationError, blocking},
  },
  session::SessionError,
  utils::time::Timestamp,
};
use crate::{
  session::resources::is_execution_id,
  storage::{
    blobs::is_blob_id,
    data_dir::DataDir,
    files::{ResourceFiles, release_resources, size, with_suffix},
  },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PruneRequest {
  /// The sessions to prune; every session when absent.
  #[serde(default)]
  sessions: Option<Vec<String>>,
  /// Unix milliseconds: only what was recorded earlier goes. Without it, everything unused goes.
  #[serde(default)]
  before: Option<u64>,
  /// Tell what would go and change nothing.
  #[serde(default)]
  dry_run: bool,
  #[serde(default)]
  archive: bool,
}

#[derive(Default)]
struct CleanupSummary {
  messages: u64,
  events: u64,
  files: u64,
  stale_entries: u64,
  unreadable_jobs: u64,
  history_bytes: u64,
  file_bytes: u64,
}
impl CleanupSummary {
  fn add(&mut self, other: &CleanupSummary) {
    self.messages += other.messages;
    self.events += other.events;
    self.files += other.files;
    self.stale_entries += other.stale_entries;
    self.unreadable_jobs += other.unreadable_jobs;
    self.history_bytes += other.history_bytes;
    self.file_bytes += other.file_bytes;
  }
  fn describe(&self) -> Value {
    json!({
      "messages": self.messages,
      "events": self.events,
      "files": self.files,
      "stale_entries": self.stale_entries,
      "unreadable_jobs": self.unreadable_jobs,
      "bytes": {"history": self.history_bytes, "files": self.file_bytes, "total": self.history_bytes + self.file_bytes},
    })
  }
}

/// Prunes each session in turn and shrinks the database once at the end. A running session is
/// skipped and reported rather than waited for.
pub async fn prune(app: Arc<App>, input: PruneRequest) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let every = input.sessions.is_none();
  let ids = match input.sessions {
    Some(ids) => ids,
    None => {
      let management = app.management.clone();
      blocking(move || {
        Ok(
          management
            .list_all()?
            .iter()
            .filter_map(|record| record["session"]["id"].as_str().map(str::to_owned))
            .collect(),
        )
      })
      .await?
    }
  };
  let (apply, before) = (!input.dry_run, input.before.map(Timestamp));
  let database = app.data_dir.database();
  let database_bytes = move || -> u64 {
    ["", "-wal"].iter().map(|suffix| size(&with_suffix(&database, suffix)).unwrap_or(0)).sum()
  };
  let initial = database_bytes();
  let (mut total, mut sessions, mut skipped) = (CleanupSummary::default(), Vec::new(), Vec::new());
  let batch = (apply && input.archive).then(|| uuid::Uuid::new_v4().to_string());
  let mut archives = vec![];
  let permit = if batch.is_some() { Some(Arc::new(super::archives::permit()?)) } else { None };
  for id in ids {
    let opening = app.lock_session_open(&id).await;
    let slot = app.cached_session(&id).await;
    let db = app.management.clone();
    let owner = id.clone();
    let record = match blocking(move || Ok(db.read(&owner)?)).await {
      Ok(record) => record,
      Err(e) if every && e.kind == super::error::ErrorKind::NotFound => continue,
      Err(e) => return Err(e.into()),
    };
    let (mut hot, publication) = if let Some(slot) = &slot {
      let Some(session) = slot.lock_idle().await else {
        skipped.push(json!({"id":id,"reason":"running"}));
        continue;
      };
      let Ok(publication) = slot.blob_publication.clone().try_write_owned() else {
        skipped.push(json!({"id":id,"reason":"uploading"}));
        continue;
      };
      (Some(session), Some(publication))
    } else {
      (None, None)
    };
    let data_dir = app.data_dir.clone();
    let target = id.clone();
    let worker = app.clone();
    let batch_id = batch.clone();
    let permit = permit.clone();
    let result = super::error::package_work(batch_id.is_some(), move || {
      let _permit = permit;
      let _opening = opening;
      let _publication = publication;
      if let Some(slot) = &slot {
        slot.require_live()?;
      }
      if slot.as_ref().is_some_and(|s| s.tools.background.is_active()) {
        return Ok(None);
      }
      if let Some(shell) = slot.as_ref().and_then(|s| s.tools.tasks()) {
        let tasks = shell.list().map_err(ApplicationError::internal)?;
        if tasks["tasks"]
          .as_array()
          .into_iter()
          .flatten()
          .any(|task| task["process"]["status"] == "running")
        {
          return Ok(None);
        }
      }
      let mut cold;
      let mut legacy_visible = HashSet::new();
      let (session, aliases) =
        if let (Some(session), Some(slot)) = (hot.as_deref_mut(), slot.as_ref()) {
          (session, slot.aliases.snapshot())
        } else {
          if apply {
            super::library::ensure(&data_dir, &worker.storage, &target)?;
          } else if !data_dir
            .blobs(&target)
            .join(".library.json")
            .try_exists()
            .map_err(ApplicationError::internal)?
          {
            legacy_visible
              .extend(super::library::recover(&data_dir, &worker.storage, &target)?.into_keys());
          }
          cold = crate::session::Session::load(worker.storage.clone(), &target)?;
          if !cold.get_state().is_stable() {
            return Ok(None);
          }
          (&mut cold, super::env_aliases::Aliases::snapshot_existing(&data_dir.blobs(&target))?)
        };
      let sender = session.create_sender();
      let _frozen = sender.freeze_publication()?;
      let references = super::library_cleanup::references(aliases);
      let history = match session.prune_history_while_frozen(
        before,
        apply && batch_id.is_none(),
        Arc::new(references.clone()),
        None,
      ) {
        Ok(history) => history,
        Err(SessionError::Busy) => return Ok(None),
        Err(error) => return Err(error.into()),
      };
      let mut protected = session.resource_dependencies(references.clone())?;
      protected.extend(legacy_visible);
      let mut plan = plan_files(
        &data_dir,
        &target,
        &history.resources,
        &protected,
        slot.as_ref().and_then(|s| s.tools.tasks()),
      )
      .map_err(ApplicationError::internal)?;
      let dir = data_dir.blobs(&target);
      let mut archive = None;
      let package_lock;
      let _package_guard;
      if let Some(batch) = batch_id {
        let hidden = crate::storage::blobs::library::release_hidden(&dir, &plan.candidates, false)?;
        plan.candidates = hidden.ids.iter().cloned().collect();
        if history.messages + history.events + hidden.ids.len() as u64 > 0
          || release_resources(&plan.resources, false)
            .map_err(ApplicationError::internal)?
            .resources
            > 0
        {
          let mut w = super::archives::Writer::new(
            &worker,
            "history_prune",
            Some(&target),
            record["session"]["name"].as_str().unwrap_or(&target),
            Some(&batch),
            false,
          )?;
          super::archives::session::records(
            &worker,
            &target,
            &mut w,
            Some(&history.selection),
            true,
          )?;
          if !hidden.ids.is_empty() {
            super::archives::library_files(&mut w, &dir, &hidden.ids)?;
          }
          let shell = data_dir.shell(&target);
          for resource in &plan.resources {
            let path = match resource {
              ResourceFiles::Directory { path } | ResourceFiles::File { path, .. } => path,
            };
            if let Ok(relative) = path.strip_prefix(&dir) {
              w.tree(&dir, relative, "blobs")?;
            } else {
              w.tree(
                &shell,
                path.strip_prefix(&shell).map_err(ApplicationError::internal)?,
                "shell",
              )?;
            }
          }
          let manifest = w.finish()?;
          package_lock = super::archives::package_lock(super::archives::package_id(&manifest)?);
          _package_guard = package_lock.lock().map_err(ApplicationError::internal)?;
          super::archives::persist(&worker, &manifest, None)?;
          if let Err(e) = session.prune_history_while_frozen(
            before,
            true,
            Arc::new(references.clone()),
            Some(history.selection),
          ) {
            super::archives::discard_unapplied(&worker, &manifest)?;
            return Err(e.into());
          }
          archive = Some(manifest);
        }
      }
      let tool_slot = slot.clone();
      let released = {
        let released =
          release_resources(&plan.resources, apply).map_err(ApplicationError::internal)?;
        let blobs = crate::storage::blobs::library::release_hidden(&dir, &plan.candidates, apply)?;
        if apply && let Some(shell) = tool_slot.as_ref().and_then(|s| s.tools.tasks()) {
          shell.forget_removed();
        }
        CleanupSummary {
          files: released.resources + blobs.unused_hidden,
          stale_entries: blobs.stale_entries,
          unreadable_jobs: plan.unreadable_jobs,
          file_bytes: released.bytes + blobs.bytes,
          ..Default::default()
        }
      };
      if apply
        && (history.messages > 0 || history.events > 0)
        && let Some(slot) = &slot
      {
        let _ = slot.events.send(json!({"type": "history_pruned"}));
      }
      Ok(Some((
        CleanupSummary {
          messages: history.messages,
          events: history.events,
          history_bytes: history.bytes,
          ..released
        },
        archive,
      )))
    })
    .await?;
    let Some((released, archive)) = result else {
      skipped.push(json!({"id": id, "reason": "running"}));
      continue;
    };
    if let Some(m) = archive {
      archives.push(m);
    }
    total.add(&released);
    let mut item = released.describe();
    item["id"] = json!(id);
    sessions.push(item);
  }
  let mut result = total.describe();
  result["sessions"] = json!(sessions);
  result["skipped"] = json!(skipped);
  if let Some(batch) = &batch {
    result["batch_id"] = json!(batch);
    result["archives"] = json!(archives);
  }
  if apply {
    if batch.is_some() {
      app.reclaim.notify_one();
    } else if total.history_bytes > 0 {
      let storage = app.storage.clone();
      blocking(move || storage.vacuum().map_err(ApplicationError::from)).await?;
    }
    result["database"] = json!({"before": initial, "after": database_bytes()});
  }
  Ok(result)
}

/// Select resource paths while the session lock is held. Running shell output is retained.
struct FilePlan {
  resources: Vec<ResourceFiles>,
  candidates: HashSet<String>,
  unreadable_jobs: u64,
}
fn plan_files(
  data_dir: &DataDir,
  id: &str,
  names: &HashSet<String>,
  protected: &HashSet<String>,
  shell: Option<&crate::task::TaskManager>,
) -> std::io::Result<FilePlan> {
  let blobs = data_dir.blobs(id);
  let executions = data_dir.shell(id);
  let mut job_resources = Vec::new();
  let mut retained_artifacts = protected.clone();
  let unreadable_jobs = super::media_generation::cleanup::visit(&blobs, |path, job| {
    if job.terminal() && job.id.as_ref().is_some_and(|id| names.contains(id)) {
      job_resources.push(ResourceFiles::File { path, companions: vec![] });
    } else {
      retained_artifacts.extend(job.blob_ids());
    }
  })?;
  // Keep recoverable job records alongside their artifacts when ownership could
  // not be established. Independent task/MCP history can still be reclaimed.
  if unreadable_jobs > 0 {
    job_resources.clear();
  }
  // MCP results use hash.ext rather than the regular blob directory.
  if let Ok(entries) = std::fs::read_dir(data_dir.mcp_files(id)) {
    for entry in entries.flatten() {
      let path = entry.path();
      if path.file_stem().and_then(|s| s.to_str()).is_some_and(|id| names.contains(id)) {
        job_resources.push(ResourceFiles::File { path, companions: vec![] });
      }
    }
  }
  let mut resources: Vec<_> = names
    .iter()
    .filter_map(|name| {
      if let Some(snapshot) =
        name.strip_prefix("snapshot:").filter(|id| uuid::Uuid::parse_str(id).is_ok())
      {
        Some(ResourceFiles::File {
          path: executions.join("web-fetch").join(format!("{snapshot}.json")),
          companions: vec![],
        })
      } else if is_execution_id(name) && !shell.is_some_and(|shell| shell.is_running(name)) {
        Some(ResourceFiles::Directory { path: executions.join(name) })
      } else {
        None
      }
    })
    .collect();
  resources.extend(job_resources);

  // Re-check visibility while holding the same lock as show/pin and remove the
  // metadata with the bytes. A concurrent add-to-Library operation cannot be lost.
  let candidates = names
    .iter()
    .filter(|name| unreadable_jobs == 0 && is_blob_id(name) && !retained_artifacts.contains(*name))
    .cloned()
    .collect();
  Ok(FilePlan { resources, candidates, unreadable_jobs })
}
