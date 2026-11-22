//! Authenticated sidebar access. The websocket receives a short-lived grant;
//! it never receives Chromium's debugging endpoint or a session shell token.
use crate::{
  application::{app::App, browser::ClientConnection},
  server::error::ApiError,
};
use axum::{
  Json,
  extract::{
    Path, Query, State, WebSocketUpgrade,
    ws::{Message, WebSocket},
  },
  response::Response,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
  collections::HashMap,
  sync::{Arc, LazyLock, Mutex, Weak},
  time::{Duration, Instant},
};
use uuid::Uuid;
struct Grant {
  app: Weak<App>,
  session: String,
  until: Instant,
}
static TICKETS: LazyLock<Mutex<HashMap<Uuid, Grant>>> = LazyLock::new(Default::default);
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
  Client,
}
#[derive(Deserialize)]
pub struct Options {
  #[serde(rename = "target")]
  _target: Target,
}
pub async fn ticket(
  State(app): State<Arc<App>>,
  Path(session): Path<String>,
  Query(_): Query<Options>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  let slot = app.get_session(&session).await?;
  slot.require_live()?;
  if !slot.get_descriptor().tools.browser {
    return Err(ApiError::conflict("browser is disabled for this session"));
  }
  let mut grants = TICKETS.lock().map_err(ApiError::internal)?;
  let now = Instant::now();
  grants.retain(|_, g| g.until > now && g.app.strong_count() > 0);
  if grants.len() >= 128 {
    return Err(ApiError::conflict("too many pending browser connections"));
  }
  let ticket = Uuid::new_v4();
  grants.insert(
    ticket,
    Grant { app: Arc::downgrade(&app), session, until: now + Duration::from_secs(30) },
  );
  Ok(Json(json!({"ticket":ticket.to_string()})))
}
#[derive(Deserialize)]
pub struct Connect {
  ticket: String,
}
pub async fn upgrade(
  State(app): State<Arc<App>>,
  Path(session): Path<String>,
  Query(q): Query<Connect>,
  ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
  let key = Uuid::parse_str(&q.ticket).map_err(|_| ApiError::unauthorized())?;
  TICKETS
    .lock()
    .map_err(ApiError::internal)?
    .remove(&key)
    .filter(|g| {
      g.session == session && g.until > Instant::now() && g.app.ptr_eq(&Arc::downgrade(&app))
    })
    .ok_or_else(ApiError::unauthorized)?;
  app.lifecycle.require_open()?;
  let slot = app.get_session(&session).await?;
  slot.require_live()?;
  if !slot.get_descriptor().tools.browser {
    return Err(ApiError::conflict("browser is disabled for this session"));
  }
  let tracked = app
    .lifecycle
    .while_open(|| app.lifecycle.tasks.token())
    .ok_or_else(|| ApiError::conflict("server is shutting down"))?;
  Ok(ws.max_message_size(65536).on_upgrade(move |mut ws| async move {
    let _tracked = tracked;
    let _ = app
      .lifecycle
      .until_shutdown(async {
        let connection = ClientConnection::attach(slot).await;
        match connection {
          Ok(connection) => run_client(&mut ws, connection).await,
          Err(e) => {
            let _ = ws.send(packet(json!({"error":e.to_string()}))).await;
          }
        }
      })
      .await;
    let _ = tokio::time::timeout(Duration::from_secs(1), ws.send(Message::Close(None))).await;
  }))
}
fn packet(value: Value) -> Message {
  Message::Text(value.to_string().into())
}

async fn run_client(ws: &mut WebSocket, mut device: ClientConnection) {
  if ws.send(packet(json!({"type":"ready"}))).await.is_err() {
    return;
  }
  let mut idle = tokio::time::Instant::now() + Duration::from_secs(60);
  let mut sequence = 0u64;
  let mut deadline = idle;
  let mut pending: Option<tokio::sync::oneshot::Sender<Result<Value, ApiError>>> = None;
  loop {
    tokio::select! {
      _ = device.closed.cancelled() => break,
      _ = tokio::time::sleep_until(idle) => break,
      _ = tokio::time::sleep_until(deadline), if pending.is_some() => {
        if let Some(reply) = pending.take() { let _ = reply.send(Err(ApiError::unknown("client browser timed out; observe before retrying"))); }
      }
      call = device.calls.recv(), if pending.is_none() => {
        let Some(call) = call else { break };
        if call.result.is_closed() { continue }
        sequence += 1;
        let message = packet(json!({"type":"command","id":sequence,"args":call.args}));
        if !matches!(tokio::time::timeout(Duration::from_secs(3), ws.send(message)).await,Ok(Ok(()))) { break }
        pending = Some(call.result);
        deadline = tokio::time::Instant::now() + Duration::from_secs(17);
        idle = tokio::time::Instant::now() + Duration::from_secs(60);
      }
      message = ws.recv() => {
        idle = tokio::time::Instant::now() + Duration::from_secs(60);
        let data = match message {
          Some(Ok(Message::Text(data))) => data,
          Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
          _ => break,
        };
        let Ok(v) = serde_json::from_str::<Value>(&data) else { break };
        if v["action"] == "ping" {
          continue;
        }
        let Some(id) = v["id"].as_u64() else { break };
        if id == 0 || id > sequence { break }
        if id < sequence { continue }
        let Some(tx) = pending.take() else { continue };
        let result = match (v.get("result"),v["error"].as_str()) {
          (_,Some(e)) => Err(ApiError::conflict(e)),
          (Some(value),None) => Ok(value.clone()),
          _ => break,
        };
        let _ = tx.send(result);
      }
    }
  }
}
