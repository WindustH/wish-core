//! What Wish keeps on disk: its data directory by kind, each session's and group's share of it, pruning what
//! sessions keep but no longer use (`prune`), and the usage records deleted sessions left
//! (`prune_usage`).
mod prune;
pub use prune::prune;

use crate::server::{
  app::App,
  data_dir::{size, with_suffix},
  error::{ApiError, blocking},
};
use crate::storage::DatabaseShape;
use axum::{Json, extract::State};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

/// Every session and group with what it stores: a session's history in the database, attachments
/// and commands' output; a group's transcript and the files posted to it; with a few facts to
/// recognize each by.
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
    let transcripts = management.transcript_sizes()?;
    let mut groups = Vec::new();
    for group in management.groups()? {
      let (messages, history) = transcripts.get(&group.id).copied().unwrap_or_default();
      let attachments = size(&data_dir.blobs(&group.id)).map_err(ApiError::internal)?;
      totals = (totals.0 + history, totals.1 + attachments, totals.2);
      groups.push(json!({
        "id": group.id,
        "name": group.name,
        "members": group.members.len(),
        "created_at": group.created_at,
        "updated_at": group.updated_at,
        "messages": messages,
        "bytes": {"history": history, "attachments": attachments, "shell": 0, "total": history + attachments},
      }));
    }
    Ok(Json(json!({
      "sessions": sessions,
      "groups": groups,
      "bytes": {"history": totals.0, "attachments": totals.1, "shell": totals.2, "total": totals.0 + totals.1 + totals.2},
    })))
  })
  .await
}
/// A database's bytes with its write-ahead log and shared memory.
fn database_size(path: &Path) -> Result<u64, ApiError> {
  let measure = |path: &Path| size(path).map_err(ApiError::internal);
  Ok(measure(path)? + measure(&with_suffix(path, "-wal"))? + measure(&with_suffix(path, "-shm"))?)
}

/// What a database holds, by kind, in order: each kind's bytes. `groups` names the kinds and the
/// tables and indexes that make them up, by a part of their names; `items` splits the engine's item
/// table by what its lists hold. What belongs to no kind is `other`; free pages and the journal -
/// what the files take beyond the database's pages: the write-ahead log and shared memory - close
/// the list, so the kinds add up to `on_disk`, the files' bytes.
fn breakdown(
  shape: &DatabaseShape,
  groups: &[(&str, &[&str])],
  items: &[(&str, &[&str])],
  on_disk: u64,
) -> Vec<Value> {
  let mut rows: Vec<(&str, u64)> = Vec::new();
  let mut counted = 0u64;
  for (kind, names) in groups {
    let bytes: u64 = shape
      .tables
      .iter()
      .filter(|(table, _)| names.iter().any(|name| table.contains(name)))
      .map(|(_, bytes)| bytes)
      .sum();
    counted += bytes;
    rows.push((kind, bytes));
  }
  // The item table is one b-tree: its pages are shared out by the bytes each kind's items take.
  if !items.is_empty() {
    let table = shape.tables.get("wish_items").copied().unwrap_or(0);
    let total: u64 = shape.items_by_kind.values().sum();
    counted += table;
    let (first, mut shared) = (rows.len(), 0);
    for (kind, list_kinds) in items {
      let payload: u64 = list_kinds.iter().filter_map(|name| shape.items_by_kind.get(*name)).sum();
      let bytes =
        if total == 0 { 0 } else { (table as u128 * payload as u128 / total as u128) as u64 };
      shared += bytes;
      rows.push((kind, bytes));
    }
    // What rounding left over goes to the first kind, so the kinds add up to the table.
    rows[first].1 += table - shared;
  }
  let free = shape.free_pages * shape.page_size;
  let pages = shape.pages * shape.page_size;
  rows.push(("other", pages.saturating_sub(counted + free)));
  rows.push(("free", free));
  rows.push(("journal", on_disk.saturating_sub(pages)));
  rows.into_iter().map(|(kind, bytes)| json!({"kind": kind, "bytes": bytes})).collect()
}

/// The bytes of what Wish keeps in its data directory - nothing else there is counted - and how
/// much of the usage records it keeps.
pub async fn storage(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let (data_dir, management) = (app.data_dir.clone(), app.management.clone());
  let limit = app.config_file.lock().await.config.usage.stream_sample_limit;
  blocking(move || {
    let measure = |path: &Path| size(path).map_err(ApiError::internal);
    let (blobs, executions) = (measure(&data_dir.blob_root())?, measure(&data_dir.shell_root())?);
    let (data, service) =
      (database_size(&data_dir.database())?, database_size(&data_dir.management_database())?);
    let total = blobs + executions + data + service;
    Ok(Json(json!({
      "bytes": {"total": total, "blobs": blobs, "executions": executions, "session_data": data, "service_data": service},
      "counts": {"executions": null, "blobs": null, "image_jobs": null, "context_generations": null},
      "stream_samples": {"count": management.stream_sample_count()?, "limit": limit},
      "leftover_usage": management.leftover_usage()?,
    })))
  })
  .await
}

/// Each database by what it holds. Reading every page of both, it is asked for apart from the
/// figures `storage` refreshes.
pub async fn detail(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let (data_dir, management, engine) =
    (app.data_dir.clone(), app.management.clone(), app.storage.clone());
  blocking(move || {
    let (data, service) =
      (database_size(&data_dir.database())?, database_size(&data_dir.management_database())?);
    let session_data = breakdown(
      &engine.measure_shape().map_err(ApiError::internal)?,
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
    let service_data = breakdown(
      &management.shape()?,
      &[
        ("sessions", &["sessions"]),
        ("groups", &["groups", "group_members"]),
        ("group_messages", &["group_messages"]),
        ("calls", &["calls"]),
        ("stream_samples", &["stream_samples"]),
      ],
      &[],
      service,
    );
    Ok(Json(json!({"session_data": session_data, "service_data": service_data})))
  })
  .await
}

/// Clears the usage records deleted sessions left - their calls and stream samples - which the
/// usage statistics count until then.
pub async fn prune_usage(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let (path, management) = (app.data_dir.management_database(), app.management.clone());
  blocking(move || {
    let before = database_size(&path)?;
    let pruned = management.prune_usage()?;
    Ok(Json(json!({
      "calls": pruned.calls,
      "stream_samples": pruned.stream_samples,
      "database": {"before": before, "after": database_size(&path)?},
    })))
  })
  .await
}
