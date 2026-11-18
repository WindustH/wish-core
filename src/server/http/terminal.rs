//! Authenticated user PTY transport; inherits the API bearer and same-origin middleware.
use crate::{
  application::{app::App, terminal_access},
  server::error::ApiError,
  task::TerminalRequest,
};
use axum::{
  Json,
  extract::{Path, State},
};
use serde_json::Value;
use std::sync::Arc;
pub async fn request(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(request): Json<TerminalRequest>,
) -> Result<Json<Value>, ApiError> {
  Ok(Json(terminal_access::execute(app, id, request).await?))
}

use axum::extract::{
  Query, WebSocketUpgrade,
  ws::{Message, WebSocket},
};
use axum::response::Response;
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use std::{
  collections::HashMap,
  sync::{LazyLock, Mutex, Weak},
  time::{Duration, Instant},
};
struct Ticket {
  app: Weak<App>,
  session: String,
  terminal: String,
  expires: Instant,
}
static TICKETS: LazyLock<Mutex<HashMap<String, Ticket>>> = LazyLock::new(Default::default);
#[derive(Deserialize)]
pub struct Connect {
  id: String,
}
/// Browser WebSockets cannot send Authorization. Exchange an authenticated POST
/// for a short-lived, single-use ticket scoped to exactly one PTY.
pub async fn ticket(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Connect>,
) -> Result<Json<Value>, ApiError> {
  terminal_access::connect(&app, &id, &input.id).await?;
  let token = uuid::Uuid::new_v4().to_string();
  let mut tickets = TICKETS.lock().map_err(ApiError::internal)?;
  tickets.retain(|_, ticket| ticket.expires > Instant::now() && ticket.app.strong_count() > 0);
  if tickets.len() >= 128 {
    return Err(ApiError::conflict("too many pending terminal connections"));
  }
  tickets.insert(
    token.clone(),
    Ticket {
      app: Arc::downgrade(&app),
      session: id,
      terminal: input.id,
      expires: Instant::now() + Duration::from_secs(30),
    },
  );
  Ok(Json(json!({"ticket":token})))
}
#[derive(Deserialize)]
pub struct Upgrade {
  ticket: String,
  #[serde(default)]
  offset: u64,
}
pub async fn upgrade(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<Upgrade>,
  ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
  let ticket = TICKETS
    .lock()
    .map_err(ApiError::internal)?
    .remove(&query.ticket)
    .filter(|t| {
      t.session == id && t.expires > Instant::now() && t.app.ptr_eq(&Arc::downgrade(&app))
    })
    .ok_or_else(ApiError::unauthorized)?;
  let terminal = terminal_access::connect(&app, &id, &ticket.terminal).await?;
  Ok(
    ws.max_message_size(65536)
      .max_frame_size(65536)
      .on_upgrade(move |socket| stream(socket, terminal, app, id, query.offset)),
  )
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Control {
  Resize { cols: u16, rows: u16 },
  Ping { time: f64 },
  Ack { offset: u64 },
}
async fn stream(
  socket: WebSocket,
  terminal: Arc<crate::task::Terminal>,
  app: Arc<App>,
  session: String,
  mut offset: u64,
) {
  terminal.attach();
  let (mut sink, mut source) = socket.split();
  let (messages, mut receive) = tokio::sync::mpsc::channel::<Message>(16);
  let stop = app.lifecycle.stop.child_token();
  let reader_stop = stop.clone();
  let reader_terminal = terminal.clone();
  let terminal_id = terminal.snapshot().id;
  let (ack, mut acknowledged) = tokio::sync::watch::channel(offset);
  let reader = async move {
    loop {
      let message = tokio::select! {
        _ = reader_stop.cancelled() => break,
        message = source.next() => match message { Some(Ok(message)) => message, _ => break }
      };
      // A socket can outlive a settings change. Keep the same live/shell gate
      // as HTTP input without putting configuration I/O on the keystroke path.
      if matches!(message, Message::Binary(_))
        && terminal_access::connect(&app, &session, &terminal_id).await.is_err()
      {
        break;
      }
      let result = match message {
        Message::Binary(bytes) => {
          let count = bytes.len();
          let result = reader_terminal.write(STANDARD.encode(bytes)).await;
          if result.is_ok()
            && messages
              .send(Message::Text(json!({"type":"input_ack","bytes":count}).to_string().into()))
              .await
              .is_err()
          {
            break;
          }
          result
        }
        Message::Text(text) => match serde_json::from_str::<Control>(&text) {
          Ok(Control::Resize { cols, rows }) => reader_terminal.resize(cols, rows),
          Ok(Control::Ping { time }) => {
            if messages
              .send(Message::Text(json!({"type":"pong","time":time}).to_string().into()))
              .await
              .is_err()
            {
              break;
            }
            Ok(())
          }
          Ok(Control::Ack { offset }) => {
            ack.send_replace(offset);
            Ok(())
          }
          Err(_) => break,
        },
        Message::Ping(data) => {
          if messages.send(Message::Pong(data)).await.is_err() {
            break;
          }
          Ok(())
        }
        Message::Pong(_) => Ok(()),
        Message::Close(_) => break,
      };
      if let Err(error) = result {
        let _ = messages
          .send(Message::Text(
            json!({"type":"error","message":error.to_string()}).to_string().into(),
          ))
          .await;
        break;
      }
    }
  };
  let writer = async {
    let mut stalled: Option<(u64, tokio::time::Instant)> = None;
    loop {
      // Bound output awaiting browser rendering. Keyboard input uses an independent reader.
      let cursor = *acknowledged.borrow();
      if offset.saturating_sub(cursor) > 131072 {
        let (_, deadline) = stalled
          .get_or_insert_with(|| (cursor, tokio::time::Instant::now() + Duration::from_secs(30)));
        let deadline = *deadline;
        tokio::select! {
          _ = stop.cancelled() => break,
          changed = acknowledged.changed() => if changed.is_err() { break },
          message = receive.recv() => if let Some(message) = message && send_frame(&mut sink, message).await.is_err() { break },
          _ = tokio::time::sleep_until(deadline) => break,
        }
        if stalled.is_some_and(|(previous, _)| *acknowledged.borrow() > previous) {
          stalled = None;
        }
        continue;
      }
      stalled = None;
      let output = tokio::select! {
        _ = stop.cancelled() => break,
        message = receive.recv() => { if let Some(message) = message && send_frame(&mut sink, message).await.is_err() { break } continue },
        output = terminal.poll(offset) => output,
      };
      let eof = !output.terminal.running && output.eof && output.data.is_empty();
      offset = output.next_offset;
      if send_frame(
        &mut sink,
        Message::Text(json!({"type":"output","output":output}).to_string().into()),
      )
      .await
      .is_err()
      {
        break;
      }
      if eof {
        break;
      }
    }
  };
  tokio::select! { _ = reader => {}, _ = writer => {}, _ = stop.cancelled() => {} }
  stop.cancel();
  // Complete the WebSocket close handshake instead of dropping a live TCP stream.
  let _ = tokio::time::timeout(Duration::from_secs(2), sink.close()).await;
  terminal.detach();
}

async fn send_frame(
  sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
  message: Message,
) -> Result<(), ()> {
  tokio::time::timeout(Duration::from_secs(30), sink.send(message))
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}
