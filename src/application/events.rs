//! Live application and session event subscriptions, independent of HTTP framing.
use super::{app::App, error::ApplicationError};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
pub async fn session_events(
  app: Arc<App>,
  id: String,
) -> Result<impl futures_util::Stream<Item = Value>, ApplicationError> {
  let slot = app.get_session(&id).await?;
  let (receiver, snapshot) = slot.subscribe_live();
  let snapshot = Some(snapshot);
  // It ends there because it holds the slot, and nothing else would ever be sent on it.
  let stream = futures_util::stream::unfold(
    (receiver, app.lifecycle.stop.clone(), snapshot, slot, false),
    |(mut receiver, stop, mut snapshot, slot, deleted)| async move {
      if deleted {
        return None;
      }
      let value = if let Some(value) = snapshot.take() {
        value
      } else {
        tokio::select! {
          _ = stop.cancelled() => return None,
          result = receiver.recv() => match result {
            Ok(value) => value,
            Err(RecvError::Lagged(_)) => {
              let (fresh, snapshot) = slot.subscribe_live();
              receiver = fresh;
              snapshot
            }
            Err(RecvError::Closed) => return None,
          }
        }
      };
      let deleted = value["type"] == "deleted";
      Some((value, (receiver, stop, snapshot, slot, deleted)))
    },
  );
  Ok(stream)
}
pub async fn application_events(app: Arc<App>) -> impl futures_util::Stream<Item = Value> {
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
      Some((value, (receiver, stop, false)))
    },
  );
  stream
}
