//! What Wish keeps on disk: its data directory by kind, each session's share of it, and pruning
//! what sessions keep but no longer use (`prune`).
mod prune;
pub use prune::prune;

use crate::server::{
  app::App,
  data_dir::{size, with_suffix},
  error::{ApiError, blocking},
};
use axum::{Json, extract::State};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

/// Every session with what it stores: its history in the database, its attachments and its
/// commands' output, with a few facts to recognize it by.
pub async fn session_storage(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let (data_dir, management, storage) =
    (app.data_dir.clone(), app.management.clone(), app.storage.clone());
  blocking(move || {
    let usage = storage.measure_children("session").map_err(ApiError::internal)?;
    let mut sessions = Vec::new();
    let mut totals = (0u64, 0u64, 0u64);
    for record in management.list_all()? {
      let session = &record["session"];
      let status = &record["status"];
      let Some(id) = session["id"].as_str() else { continue };
      let stored = usage.get(&hex::encode(id.as_bytes())).cloned().unwrap_or_default();
      let attachments = size(&data_dir.blobs(id)).map_err(ApiError::internal)?;
      let shell = size(&data_dir.shell(id)).map_err(ApiError::internal)?;
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
    Ok(Json(json!({
      "sessions": sessions,
      "bytes": {"history": totals.0, "attachments": totals.1, "shell": totals.2, "total": totals.0 + totals.1 + totals.2},
    })))
  })
  .await
}
/// The bytes of what Wish keeps in its data directory; nothing else there is counted.
pub async fn storage(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let data_dir = app.data_dir.clone();
  blocking(move || {
    let measure = |path: &Path| size(path).map_err(ApiError::internal);
    let database = |path: &Path| -> Result<u64, ApiError> {
      Ok(measure(path)? + measure(&with_suffix(path, "-wal"))? + measure(&with_suffix(path, "-shm"))?)
    };
    let (blobs, executions) = (measure(&data_dir.blob_root())?, measure(&data_dir.shell_root())?);
    let (data, service) =
      (database(&data_dir.database())?, database(&data_dir.management_database())?);
    let total = blobs + executions + data + service;
    Ok(Json(json!({
      "bytes": {"total": total, "blobs": blobs, "executions": executions, "session_data": data, "service_data": service},
      "counts": {"executions": null, "blobs": null, "image_jobs": null, "context_generations": null},
    })))
  })
  .await
}
