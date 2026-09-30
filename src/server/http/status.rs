//! The service as a whole: its status counts, and the event stream that tells a client when the
//! session list or the configuration changed.
use crate::server::{
  app::App,
  error::{ApiError, blocking},
};
use crate::session::SessionPhase;
use axum::{
  Json,
  extract::State,
  response::{
    Sse,
    sse::{Event, KeepAlive},
  },
};
use serde_json::{Value, json};
use std::{convert::Infallible, sync::Arc};
use tokio::sync::broadcast::error::RecvError;

pub async fn status(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let (mut active, mut ready, mut compacting, mut pending) = (0, 0, 0, 0);
  for slot in app.sessions.lock().await.values() {
    let status = slot.get_status();
    active += i64::from(status.status.running);
    ready += i64::from(status.status.phase == SessionPhase::Ready);
    compacting += i64::from(status.status.phase == SessionPhase::Compacting);
    pending += status.queue_count;
  }
  let management = app.management.clone();
  let sessions = blocking(move || management.count()).await?;
  Ok(Json(json!({
    "counts": {"sessions": sessions, "runs": null},
    "queue": {
      "active_sessions": active,
      "ready_sessions": ready,
      "pending_items": pending,
      "compacting_sessions": compacting,
    },
    "uptime_ms": app.started.elapsed().as_millis(),
  })))
}

/// A `snapshot` first, then each change as it happens; a client that fell behind is sent a `gap`
/// and reloads.
pub async fn events(
  State(app): State<Arc<App>>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
  let receiver = app.events.subscribe();
  let stop = app.lifecycle.stop.clone();
  let stream = futures_util::stream::unfold(
    (receiver, stop, true),
    |(mut receiver, stop, first)| async move {
      let value = if first {
        json!({"type":"snapshot"})
      } else {
        tokio::select! {
          _ = stop.cancelled() => return None,
          result = receiver.recv() => match result {
            Ok(value) => value,
            Err(RecvError::Lagged(_)) => json!({"type":"gap"}),
            Err(_) => return None,
          }
        }
      };
      Some((Ok(Event::default().event("wish").data(value.to_string())), (receiver, stop, false)))
    },
  );
  Sse::new(stream).keep_alive(KeepAlive::default())
}
