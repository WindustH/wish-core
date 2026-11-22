//! Disk usage views combining persisted database measurements and resource sizes.
use crate::application::app::App;
use crate::application::error::{ApplicationError, blocking};
use crate::storage::{
  files::{database_size, size},
  measurement::database_breakdown,
};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

pub async fn session_preview(app: Arc<App>, id: String) -> Result<Value, ApplicationError> {
  blocking(move || {
    app.management.read(&id)?;
    let messages = crate::session::SessionReader::preview(&app.storage, &id)?;
    Ok(json!({"messages":messages}))
  })
  .await
}

/// Every session with what it stores: a session's history in the database, attachments
/// and commands' output; with a few facts to
/// recognize each by.
pub async fn session_storage(app: Arc<App>) -> Result<Value, ApplicationError> {
  let (data_dir, management, storage) =
    (app.data_dir.clone(), app.management.clone(), app.storage.clone());
  blocking(move || {
    let usage = storage.measure_children("session").map_err(ApplicationError::internal)?;
    let mut sessions = Vec::new();
    let mut totals = (0u64, 0u64, 0u64);
    for record in management.list_all()? {
      let session = &record["session"];
      let status = &record["status"];
      let Some(id) = session["id"].as_str() else { continue };
      let stored = usage.get(&hex::encode(id.as_bytes())).cloned().unwrap_or_default();
      let attachments = size(&data_dir.blobs(id)).map_err(ApplicationError::internal)?;
      let shell = size(&data_dir.shell(id)).map_err(ApplicationError::internal)?;
      totals = (totals.0 + stored.bytes, totals.1 + attachments, totals.2 + shell);
      let count = |kind: &str| stored.messages.get(kind).copied().unwrap_or(0);
      sessions.push(json!({
        "id": id,
        "name": session["name"],
        "provider": session["provider"],
        "model": status["config"]["model"],
        "cwd": session["cwd"],
        "created_at": session["created_at"],
        "updated_at": session["updated_at"],
        "phase": status["phase"],
        "running": status["running"],
        "tags": status["metadata"]["tags"],
        "context_tokens": status["context_tokens"],
        "messages": {"user": count("user"), "assistant": count("assistant"), "tool_calls": count("tool_use")},
        "bytes": {"history": stored.bytes, "attachments": attachments, "shell": shell, "total": stored.bytes + attachments + shell},
      }));
    }
    Ok(json!({
      "sessions": sessions,
      "bytes": {"history": totals.0, "attachments": totals.1, "shell": totals.2, "total": totals.0 + totals.1 + totals.2},
    }))
  })
  .await
}
/// The bytes of what Wish keeps in its data directory - nothing else there is counted - and how
/// much of the usage records it keeps.
pub async fn storage(app: Arc<App>) -> Result<Value, ApplicationError> {
  let (data_dir, management) = (app.data_dir.clone(), app.management.clone());
  let limit = app.config_file.lock().await.config.usage.stream_sample_limit;
  blocking(move || {
    let measure = |path: &Path| size(path).map_err(ApplicationError::internal);
    let (blobs, executions) = (measure(&data_dir.blob_root())?, measure(&data_dir.shell_root())?);
    let (data, service) =
      (database_size(&data_dir.database()).map_err(ApplicationError::internal)?, database_size(&data_dir.management_database()).map_err(ApplicationError::internal)?);
    let archives=measure(&data_dir.packages(false))?;
    let total = blobs + executions + data + service + archives;
    Ok(json!({
      "bytes": {"total": total, "archives":archives, "blobs": blobs, "executions": executions, "session_data": data, "service_data": service},
      "counts": {"executions": null, "blobs": null, "image_jobs": null, "context_generations": null},
      "stream_samples": {"count": management.stream_sample_count()?, "limit": limit},
      "leftover_usage": management.leftover_usage()?,
    }))
  })
  .await
}

/// Each database by what it holds. Reading every page of both, it is asked for apart from the
/// figures `storage` refreshes.
pub async fn detail(app: Arc<App>) -> Result<Value, ApplicationError> {
  let (data_dir, management, engine) =
    (app.data_dir.clone(), app.management.clone(), app.storage.clone());
  blocking(move || {
    let (data, service) = (
      database_size(&data_dir.database()).map_err(ApplicationError::internal)?,
      database_size(&data_dir.management_database()).map_err(ApplicationError::internal)?,
    );
    let session_data = database_breakdown(
      &engine.measure_shape().map_err(ApplicationError::internal)?,
      &[
        ("sessions", &["wish_objects"]),
        ("history_index", &["wish_history_index", "wish_history_by_"]),
        ("search_index", &["wish_history_words", "wish_history_substrings"]),
      ],
      &[
        ("messages", &["entries"]),
        ("events", &["events"]),
        ("history", &["history"]),
        ("context", &["generations", "lists"]),
        ("model_calls", &["model_calls"]),
        ("queue", &["queue"]),
      ],
      data,
    );
    let service_data = database_breakdown(
      &management.shape()?,
      &[
        ("sessions", &["sessions"]),
        ("calls", &["calls"]),
        ("stream_samples", &["stream_samples"]),
      ],
      &[],
      service,
    );
    Ok(json!({"session_data": session_data, "service_data": service_data, "archives":size(&data_dir.packages(false)).map_err(ApplicationError::internal)?}))
  })
  .await
}

/// Clears the usage records deleted sessions left - their calls and stream samples - which the
/// usage statistics count until then.
pub async fn prune_usage(app: Arc<App>, archive: bool) -> Result<Value, ApplicationError> {
  let (path, management) = (app.data_dir.management_database(), app.management.clone());
  let permit = if archive { Some(super::archives::permit()?) } else { None };
  super::error::package_work(archive,move || {
    let _permit = permit;
    let before = database_size(&path).map_err(ApplicationError::internal)?;
    if archive {
      let (calls,samples,manifest)=super::archives::usage(&app,&path)?;
      let after=database_size(&path).map_err(ApplicationError::internal)?;
      return Ok(json!({"calls":calls,"stream_samples":samples,"archive":manifest,"database":{"before":before,"after":after}}));
    }
    let pruned = management.prune_usage()?;
    Ok(json!({
      "calls": pruned.calls,
      "stream_samples": pruned.stream_samples,
      "database": {"before": before, "after": database_size(&path).map_err(ApplicationError::internal)?},
    }))
  })
  .await
}
