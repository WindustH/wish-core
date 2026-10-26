//! Releasing what sessions keep but no longer use: the history their context has moved past, and
//! the attachments, images and command output only that history named.
use crate::{
  application::{
    app::App,
    error::{ApplicationError, blocking},
  },
  session::SessionError,
  tool::shell::ShellTool,
  utils::time::Timestamp,
};
use crate::{
  session::resources::{collect_references, is_execution_id},
  storage::{
    blobs::{description_path, is_blob_id},
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
}

#[derive(Default)]
struct CleanupSummary {
  messages: u64,
  events: u64,
  files: u64,
  history_bytes: u64,
  file_bytes: u64,
}
impl CleanupSummary {
  fn add(&mut self, other: &CleanupSummary) {
    self.messages += other.messages;
    self.events += other.events;
    self.files += other.files;
    self.history_bytes += other.history_bytes;
    self.file_bytes += other.file_bytes;
  }
  fn describe(&self) -> Value {
    json!({
      "messages": self.messages,
      "events": self.events,
      "files": self.files,
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
  for id in ids {
    let slot = match app.get_session(&id).await {
      Ok(slot) => slot,
      // A session deleted while this ran is simply gone.
      Err(error) if every && error.kind == crate::application::error::ErrorKind::NotFound => {
        continue;
      }
      Err(error) => return Err(error),
    };
    let Some(session) = slot.lock_idle().await else {
      skipped.push(json!({"id": id, "reason": "running"}));
      continue;
    };
    let data_dir = app.data_dir.clone();
    let target = id.clone();
    let result = blocking(move || {
      let mut session = session;
      slot.require_live()?;
      if slot.tools.background.is_active() { return Ok(None); }
      if let Some(shell) = slot.tools.shell() {
        let tasks = shell.list().map_err(ApplicationError::internal)?;
        if tasks["tasks"].as_array().into_iter().flatten().any(|task| task["process"]["status"] == "running") {
          return Ok(None);
        }
      }
      let history = match session.prune_history(before, apply, collect_references) {
        Ok(history) => history,
        Err(SessionError::Busy) => return Ok(None),
        Err(error) => return Err(error.into()),
      };
      let (files, file_bytes) = release_unreferenced_files(
        &data_dir,
        &target,
        &history.resources,
        slot.tools.shell(),
        apply,
      )
      .map_err(ApplicationError::internal)?;
      if apply && (history.messages > 0 || history.events > 0) {
        let _ = slot.events.send(json!({"type": "history_pruned"}));
      }
      Ok(Some(CleanupSummary {
        messages: history.messages,
        events: history.events,
        files,
        history_bytes: history.bytes,
        file_bytes,
      }))
    })
    .await?;
    let Some(released) = result else {
      skipped.push(json!({"id": id, "reason": "running"}));
      continue;
    };
    total.add(&released);
    let mut item = released.describe();
    item["id"] = json!(id);
    sessions.push(item);
  }
  let mut result = total.describe();
  result["sessions"] = json!(sessions);
  result["skipped"] = json!(skipped);
  if apply {
    if total.history_bytes > 0 {
      let storage = app.storage.clone();
      blocking(move || storage.vacuum().map_err(ApplicationError::from)).await?;
    }
    result["database"] = json!({"before": initial, "after": database_bytes()});
  }
  Ok(result)
}

/// Select resource paths while the session lock is held. Running shell output is retained.
fn release_unreferenced_files(
  data_dir: &DataDir,
  id: &str,
  names: &HashSet<String>,
  shell: Option<&ShellTool>,
  apply: bool,
) -> std::io::Result<(u64, u64)> {
  let blobs = data_dir.blobs(id);
  let executions = data_dir.shell(id);
  let resources: Vec<_> = names
    .iter()
    .filter_map(|name| {
      if is_blob_id(name) {
        Some(ResourceFiles::File {
          path: blobs.join(name),
          companions: vec![description_path(&blobs, name)],
        })
      } else if is_execution_id(name) && !shell.is_some_and(|shell| shell.is_running(name)) {
        Some(ResourceFiles::Directory { path: executions.join(name) })
      } else {
        None
      }
    })
    .collect();
  let released = release_resources(&resources, apply)?;
  Ok((released.resources, released.bytes))
}
