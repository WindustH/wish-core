//! Runtime counters for sessions, operations, queues and process uptime.
use crate::application::app::App;
use crate::application::error::{ApplicationError, blocking};
use crate::session::SessionPhase;
use serde_json::{Value, json};
use std::sync::Arc;

pub async fn status(app: Arc<App>) -> Result<Value, ApplicationError> {
  let (mut active, mut ready, mut compacting, mut pending) = (0, 0, 0, 0);
  for slot in app.sessions.lock().await.values() {
    let status = slot.get_status();
    active += i64::from(status.status.running);
    ready += i64::from(status.status.phase == SessionPhase::Ready);
    compacting += i64::from(status.status.phase == SessionPhase::Compacting);
    pending += status.queue_count;
  }
  let management = app.management.clone();
  let sessions = blocking(move || Ok(management.count()?)).await?;
  Ok(json!({
    "counts": {"sessions": sessions, "runs": null},
    "queue": {
      "active_sessions": active,
      "ready_sessions": ready,
      "pending_items": pending,
      "compacting_sessions": compacting,
    },
    "uptime_ms": app.started.elapsed().as_millis(),
  }))
}
