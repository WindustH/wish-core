//! HTTP response framing for a temporary session question.
use super::sse;
use crate::application::{
  app::App,
  session_questions::{self, Ask},
};
use crate::server::error::ApiError;
use axum::{
  Json,
  extract::{Path, State},
  response::Response,
};
use std::sync::Arc;
pub async fn ask(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Json(input): Json<Ask>,
) -> Result<Response, ApiError> {
  let stop = app.lifecycle.stop.clone();
  Ok(sse::model_response(session_questions::ask(app, id, input).await?, stop))
}
