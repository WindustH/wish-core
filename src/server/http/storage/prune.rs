//! Releasing what sessions keep but no longer use: the history their context has moved past, and
//! the attachments, images and command output only that history named.
use crate::{
  protocol::Message,
  server::{
    app::App,
    blobs::{blob_id, description_path, is_blob_id},
    data_dir::{DataDir, size, with_suffix},
    error::{ApiError, blocking},
  },
  session::SessionError,
  tool::shell::ShellTool,
  utils::time::Timestamp,
};
use axum::{Json, extract::State};
use base64::Engine;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prune {
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
struct Released {
  messages: u64,
  events: u64,
  files: u64,
  history_bytes: u64,
  file_bytes: u64,
}
impl Released {
  fn add(&mut self, other: &Released) {
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
pub async fn prune(
  State(app): State<Arc<App>>,
  Json(input): Json<Prune>,
) -> Result<Json<Value>, ApiError> {
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
  let (mut total, mut sessions, mut skipped) = (Released::default(), Vec::new(), Vec::new());
  for id in ids {
    let slot = match app.get_session(&id).await {
      Ok(slot) => slot,
      // A session deleted while this ran is simply gone.
      Err(error) if every && error.status == axum::http::StatusCode::NOT_FOUND => continue,
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
      let history = match session.prune_history(before, apply, references) {
        Ok(history) => history,
        Err(SessionError::Busy) => return Ok(None),
        Err(error) => return Err(error.into()),
      };
      let (files, file_bytes) =
        release_files(&data_dir, &target, &history.resources, slot.tools.shell(), apply)
          .map_err(ApiError::internal)?;
      if apply && (history.messages > 0 || history.events > 0) {
        let _ = slot.events.send(json!({"type": "history_pruned"}));
      }
      Ok(Some(Released {
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
      blocking(move || storage.vacuum().map_err(ApiError::from)).await?;
    }
    result["database"] = json!({"before": initial, "after": database_bytes()});
  }
  Ok(Json(result))
}

/// The session files a message names: blobs by the SHA-256 of their bytes, which is also what an
/// inline image hashes to, and shell executions by their id. Any word shaped like one counts;
/// only those on disk matter.
fn references(message: &Message, found: &mut HashSet<String>) {
  if let Ok(value) = serde_json::to_value(message) {
    collect(&value, found);
  }
}
fn collect(value: &Value, found: &mut HashSet<String>) {
  match value {
    Value::String(text) => {
      for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        if is_blob_id(word) || is_execution(word) {
          found.insert(word.to_owned());
        }
      }
    }
    Value::Array(items) => items.iter().for_each(|item| collect(item, found)),
    Value::Object(fields) => {
      for (key, item) in fields {
        match (key.as_str(), item) {
          ("data_base64", Value::String(data)) => {
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
              found.insert(blob_id(&bytes));
            }
          }
          _ => collect(item, found),
        }
      }
    }
    _ => {}
  }
}
/// A shell execution id: `<pid>-<nanoseconds>-<counter>`.
fn is_execution(word: &str) -> bool {
  let parts: Vec<_> = word.split('-').collect();
  parts.len() == 3
    && parts.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Remove, or with `apply` false only measure, the files `names` point at: a blob with its
/// description, or a finished execution's output. Returns how many and their bytes.
fn release_files(
  data_dir: &DataDir,
  id: &str,
  names: &HashSet<String>,
  shell: Option<&ShellTool>,
  apply: bool,
) -> std::io::Result<(u64, u64)> {
  let (blobs, executions) = (data_dir.blobs(id), data_dir.shell(id));
  let (mut count, mut bytes) = (0, 0);
  for name in names {
    if is_blob_id(name) {
      let paths = [blobs.join(name), description_path(&blobs, name)];
      if !paths[0].is_file() {
        continue;
      }
      count += 1;
      for path in paths {
        bytes += size(&path)?;
        if apply {
          remove(std::fs::remove_file(&path))?;
        }
      }
    } else if is_execution(name) {
      let path = executions.join(name);
      if !path.is_dir() || shell.is_some_and(|shell| shell.is_running(name)) {
        continue;
      }
      count += 1;
      bytes += size(&path)?;
      if apply {
        remove(std::fs::remove_dir_all(&path))?;
      }
    }
  }
  Ok((count, bytes))
}
fn remove(result: std::io::Result<()>) -> std::io::Result<()> {
  match result {
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
    result => result,
  }
}
