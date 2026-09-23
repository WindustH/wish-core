use crate::server::{
  app::App,
  error::{ApiError, blocking},
  session::SessionSlot,
};
use axum::{
  Json,
  body::Bytes,
  extract::{Path, Query, State},
  http::{HeaderMap, StatusCode, header},
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use crate::{
  protocol::{ContentBlock, Message},
  session::history::query::{
    HistoryCursor, HistoryFilter, HistoryKind, HistoryOrder, HistoryPageRequest,
  },
};
#[derive(Serialize, Deserialize)]
pub struct Blob {
  pub id: String,
  pub mime_type: String,
  pub byte_count: usize,
  pub path: String,
}
fn valid_blob(id: &str) -> bool {
  id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
pub async fn upload(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  body: Bytes,
) -> Result<Json<Blob>, ApiError> {
  app.require_open()?;
  app.get_session(&id).await?.require_live()?;
  let hash = format!("{:x}", Sha256::digest(&body));
  let dir = app.data_dir.join("blobs").join(&id);
  tokio::fs::create_dir_all(&dir).await.map_err(ApiError::internal)?;
  let path = dir.join(&hash);
  tokio::fs::write(&path, &body).await.map_err(ApiError::internal)?;
  let mime = if body.starts_with(b"\x89PNG\r\n\x1a\n") {
    "image/png"
  } else if body.starts_with(b"\xff\xd8\xff") {
    "image/jpeg"
  } else if body.starts_with(b"GIF8") {
    "image/gif"
  } else if body.len() >= 12 && &body[..4] == b"RIFF" && &body[8..12] == b"WEBP" {
    "image/webp"
  } else {
    "application/octet-stream"
  };
  let blob = Blob {
    id: hash.clone(),
    mime_type: mime.into(),
    byte_count: body.len(),
    path: tokio::fs::canonicalize(&path)
      .await
      .map_err(ApiError::internal)?
      .to_string_lossy()
      .into_owned(),
  };
  tokio::fs::write(dir.join(format!("{hash}.json")), serde_json::to_vec(&blob).unwrap())
    .await
    .map_err(ApiError::internal)?;
  Ok(Json(blob))
}
pub async fn download(
  State(app): State<Arc<App>>,
  Path((id, blob)): Path<(String, String)>,
) -> Result<(HeaderMap, Vec<u8>), ApiError> {
  app.get_session(&id).await?.require_live()?;
  if !valid_blob(&blob) {
    return Err(ApiError::not_found());
  }
  let body =
    tokio::fs::read(app.data_dir.join("blobs").join(id).join(blob)).await.map_err(|e| {
      if e.kind() == std::io::ErrorKind::NotFound {
        ApiError::not_found()
      } else {
        ApiError::internal(e)
      }
    })?;
  let mut headers = HeaderMap::new();
  headers.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
  headers
    .insert(header::HeaderName::from_static("x-content-type-options"), "nosniff".parse().unwrap());
  Ok((headers, body))
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
  id: String,
  kind: String,
  name: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
  pub text: String,
  #[serde(default)]
  pub attachments: Vec<Attachment>,
  #[serde(default)]
  pub metadata: Value,
}
pub async fn input(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Input>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
  app.require_open()?;
  let slot = app.get_session(&id).await?;
  slot.require_live()?;
  let mut content = Vec::new();
  if !input.text.is_empty() {
    content.push(ContentBlock::Text { text: input.text.clone() });
  }
  for attachment in &input.attachments {
    if !valid_blob(&attachment.id) {
      return Err(ApiError::bad_request("invalid attachment id"));
    }
    let dir = app.data_dir.join("blobs").join(&id);
    let blob: Blob = serde_json::from_slice(
      &tokio::fs::read(dir.join(format!("{}.json", attachment.id)))
        .await
        .map_err(|_| ApiError::not_found())?,
    )
    .map_err(ApiError::internal)?;
    match attachment.kind.as_str() {
      "image" => {
        if !blob.mime_type.starts_with("image/") {
          return Err(ApiError::bad_request("attachment is not a supported image"));
        }
        let bytes = tokio::fs::read(&blob.path).await.map_err(ApiError::internal)?;
        content.push(ContentBlock::Image {
          mime_type: blob.mime_type,
          data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        });
      }
      "file" => content.push(ContentBlock::Text {
        text: format!("Attached file {}: {}", json!(attachment.name), json!(blob.path)),
      }),
      _ => return Err(ApiError::bad_request("attachment kind must be image or file")),
    }
  }
  if content.is_empty() {
    return Err(ApiError::bad_request("message is empty"));
  }
  let message = Message::User {
    metadata: json!({"custom":input.metadata,"attachments":input.attachments,"input_text":input.text}),
    content,
  };
  let owner = slot.clone();
  let entry = blocking(move || Ok(owner.handle.enqueue_message(message)?)).await?;
  let _ = app.events.send(json!({"type":"session_changed","id":id}));
  schedule(app, slot);
  Ok((StatusCode::ACCEPTED, Json(json!({"entry":entry}))))
}
pub(crate) fn schedule(app: Arc<App>, slot: Arc<SessionSlot>) {
  let generation = slot.auto_run_generation.load(std::sync::atomic::Ordering::SeqCst);
  let tasks = app.tasks.clone();
  tasks.spawn(async move {
    let mut session = slot.session.clone().lock_owned().await;
    let setup = (|| -> Result<_, ApiError> {
      let closing = app.closing.lock().unwrap();
      if *closing
        || generation != slot.auto_run_generation.load(std::sync::atomic::Ordering::SeqCst)
      {
        return Ok(None);
      }
      slot.require_live()?;
      if session.get_queue_head() >= session.get_message_queue().len()? {
        return Ok(None);
      }
      if !session.get_state().is_stable() {
        if slot.control.lock().unwrap().is_none() {
          session.settle_interrupted().map_err(ApiError::internal)?;
          slot.update_snapshot(&session);
        } else {
          return Err(ApiError::conflict("session has unfinished execution"));
        }
      }
      let provider = app.get_provider(&slot.get_descriptor().provider)?;
      session.resume()?;
      slot.update_snapshot(&session);
      let control = crate::executor::ExecutionControl::new();
      *slot.control.lock().unwrap() = Some(control.clone());
      {
        let mut status = slot.status.lock().unwrap();
        status["running"] = json!(true);
        status.as_object_mut().unwrap().remove("state");
      }
      slot.persist_index()?;
      Ok(Some((provider, control)))
    })();
    match setup {
      Ok(Some((provider, control))) => slot.execute(provider, session, control, false).await,
      Ok(None) => {}
      Err(e) => {
        let _ = slot.events.send(json!({"type":"operation_failed","error":e.to_string()}));
      }
    }
  });
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Timeline {
  include_outcomes: bool,
  limit: usize,
  before: Option<u64>,
  after: Option<u64>,
  order: String,
}
impl Default for Timeline {
  fn default() -> Self {
    Self { include_outcomes: false, limit: 40, before: None, after: None, order: "desc".into() }
  }
}
pub async fn timeline(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<Timeline>,
) -> Result<Json<Value>, ApiError> {
  let reader = app.get_session(&id).await?.history.clone();
  blocking(move || {
    let order = match query.order.as_str() {
      "asc" => HistoryOrder::OldestFirst,
      "desc" => HistoryOrder::NewestFirst,
      _ => return Err(ApiError::bad_request("invalid order")),
    };
    let base = reader.query_history(
      HistoryFilter { kind: Some(HistoryKind::Message), ..Default::default() },
      HistoryPageRequest { limit: 1, ..Default::default() },
    )?;
    let cursor = if let Some(before) = query.before {
      Some(HistoryCursor {
        end_sequence: before.min(base.end_sequence),
        after_sequence: before,
        order,
      })
    } else {
      query.after.map(|after| HistoryCursor {
        end_sequence: base.end_sequence,
        after_sequence: after,
        order,
      })
    };
    let page = reader.query_history(
      HistoryFilter { kind: Some(HistoryKind::Message), ..Default::default() },
      HistoryPageRequest { limit: query.limit, order, cursor: cursor.clone() },
    )?;
    let mut matches = page.items;
    let mut has_more = page.next.is_some();
    if query.include_outcomes {
      let outcomes = reader.query_history(
        HistoryFilter { kind: Some(HistoryKind::Event), event_types: vec!["Finished".into()], ..Default::default() },
        HistoryPageRequest { limit: query.limit, order, cursor },
      )?;
      has_more |= outcomes.next.is_some();
      matches.extend(outcomes.items);
      matches.sort_by_key(|item| item.record.sequence);
      if order == HistoryOrder::NewestFirst { matches.reverse(); }
      has_more |= matches.len() > query.limit;
      matches.truncate(query.limit);
    }
    let next = if has_more {
      matches.last().map(|item| HistoryCursor { end_sequence: page.end_sequence, after_sequence: item.record.sequence, order })
    } else { None };
    let mut items = Vec::new();
    for item in &matches {
      if let Some(item) = reader.read_history_item(item.record.sequence)? {
        items.push(item);
      }
    }
    Ok(Json(json!({"items":items,"next":next,"end_sequence":page.end_sequence})))
  })
  .await
}
