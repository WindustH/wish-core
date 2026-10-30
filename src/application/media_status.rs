//! User-facing recovery of an existing session media job; never creates a generation.
use super::{app::App, error::ApplicationError};
use serde_json::Value;
use std::sync::Arc;
pub async fn status(
  app: Arc<App>,
  session: String,
  kind: String,
  job: String,
) -> Result<Value, ApplicationError> {
  let _slot = app.get_session(&session).await?;
  let (hub, video) = match kind.as_str() {
    "video" => (&app.videos, true),
    "audio" => (&app.audio, false),
    _ => return Err(ApplicationError::not_found()),
  };
  let directory = app.data_dir.blobs(&session);
  let accounts = app.get_service_accounts();
  app
    .lifecycle
    .until_shutdown(hub.recover(job, &directory, &accounts, video))
    .await?
    .map_err(ApplicationError::invalid)
}
