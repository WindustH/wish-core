//! The error every handler answers with, how the engine's errors map onto it, and the rule for
//! running storage work from async code.
use crate::{session::SessionError, storage::StorageError};
use axum::{
  Json,
  http::StatusCode,
  response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug)]
pub struct ApiError {
  pub status: StatusCode,
  pub message: String,
  pub details: Option<serde_json::Value>,
}
impl ApiError {
  pub fn bad_request(message: impl Into<String>) -> Self {
    Self { status: StatusCode::BAD_REQUEST, message: message.into(), details: None }
  }
  pub fn conflict(message: impl Into<String>) -> Self {
    Self { status: StatusCode::CONFLICT, message: message.into(), details: None }
  }
  pub fn not_found() -> Self {
    Self { status: StatusCode::NOT_FOUND, message: "not found".into(), details: None }
  }
  pub fn unauthorized() -> Self {
    Self { status: StatusCode::UNAUTHORIZED, message: "unauthorized".into(), details: None }
  }
  pub fn internal(error: impl std::fmt::Display) -> Self {
    Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: error.to_string(), details: None }
  }
  /// Shutdown has begun: nothing new starts, and waits in progress end.
  pub fn shutting_down() -> Self {
    Self::conflict("server is shutting down")
  }
}
impl std::fmt::Display for ApiError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", self.message)
  }
}
impl std::error::Error for ApiError {}
impl IntoResponse for ApiError {
  fn into_response(self) -> Response {
    (self.status, Json(json!({"error":{"message":self.message,"details":self.details}})))
      .into_response()
  }
}
impl From<SessionError> for ApiError {
  fn from(error: SessionError) -> Self {
    match error {
      SessionError::Storage(e) => e.into(),
      SessionError::Busy | SessionError::UnexpectedPhase | SessionError::Suspended => {
        Self::conflict(error.to_string())
      }
      _ => Self::bad_request(error.to_string()),
    }
  }
}
impl From<StorageError> for ApiError {
  fn from(error: StorageError) -> Self {
    match error {
      StorageError::NotFound(_) => Self::not_found(),
      StorageError::AlreadyExists(_) | StorageError::AlreadyOpen(_) => {
        Self::conflict(error.to_string())
      }
      StorageError::InvalidRange => Self::bad_request(error.to_string()),
      _ => Self::internal(error),
    }
  }
}
impl From<rusqlite::Error> for ApiError {
  fn from(error: rusqlite::Error) -> Self {
    Self::internal(error)
  }
}
impl From<crate::tool::ask_user::AnswerError> for ApiError {
  fn from(error: crate::tool::ask_user::AnswerError) -> Self {
    use crate::tool::ask_user::AnswerError;
    match error {
      AnswerError::NotFound => Self::not_found(),
      AnswerError::Invalid(_) => Self::bad_request(error.to_string()),
      AnswerError::Closed(_) => Self::conflict(error.to_string()),
    }
  }
}
impl From<crate::Error> for ApiError {
  fn from(error: crate::Error) -> Self {
    let status = match &error {
      crate::Error::Build(_) => StatusCode::BAD_REQUEST,
      crate::Error::Unsupported { .. } => StatusCode::NOT_IMPLEMENTED,
      _ => StatusCode::BAD_GATEWAY,
    };
    Self { status, message: error.to_string(), details: Some(json!(error)) }
  }
}

/// Runs storage work on a thread of its own, off the async workers.
///
/// The rule: request handlers read and write the databases - `wish.sqlite` through a session or the
/// storage, `management.sqlite` through the index - inside `blocking`. The one exception is saving
/// a session's index record where the session changes (`SessionSlot::persist_index`, and the model
/// calls saved beside it when an operation ends): a single small write, which the engine's event
/// callbacks make too, and they cannot await.
pub async fn blocking<T: Send + 'static>(
  f: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
  tokio::task::spawn_blocking(f).await.map_err(ApiError::internal)?
}
