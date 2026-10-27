//! Application failures independent of the HTTP adapter, and blocking persistence execution.
use crate::{session::SessionError, storage::StorageError};
use serde_json::json;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
  InvalidInput,
  Conflict,
  NotFound,
  Unauthorized,
  Forbidden,
  Internal,
  Unsupported,
  Upstream,
  Rejected,
  Timeout,
}
#[derive(Debug)]
pub struct ApplicationError {
  pub kind: ErrorKind,
  pub message: String,
  pub details: Option<serde_json::Value>,
}
impl ApplicationError {
  pub fn invalid(message: impl Into<String>) -> Self {
    Self { kind: ErrorKind::InvalidInput, message: message.into(), details: None }
  }
  pub fn conflict(message: impl Into<String>) -> Self {
    Self { kind: ErrorKind::Conflict, message: message.into(), details: None }
  }
  pub fn not_found() -> Self {
    Self { kind: ErrorKind::NotFound, message: "not found".into(), details: None }
  }
  pub fn unauthorized() -> Self {
    Self { kind: ErrorKind::Unauthorized, message: "unauthorized".into(), details: None }
  }
  pub fn internal(error: impl std::fmt::Display) -> Self {
    Self { kind: ErrorKind::Internal, message: error.to_string(), details: None }
  }
  /// An external operation may have taken effect; callers must inspect state before retrying.
  pub fn unknown(message: impl Into<String>) -> Self {
    Self {
      kind: ErrorKind::Internal,
      message: message.into(),
      details: Some(json!({"kind":"unknown"})),
    }
  }
  /// Shutdown has begun: nothing new starts, and waits in progress end.
  pub fn shutting_down() -> Self {
    Self::conflict("server is shutting down")
  }
}
impl std::fmt::Display for ApplicationError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", self.message)
  }
}
impl std::error::Error for ApplicationError {}
impl From<SessionError> for ApplicationError {
  fn from(error: SessionError) -> Self {
    match error {
      SessionError::Storage(e) => e.into(),
      SessionError::Busy | SessionError::UnexpectedPhase | SessionError::Suspended => {
        Self::conflict(error.to_string())
      }
      _ => Self::invalid(error.to_string()),
    }
  }
}
impl From<StorageError> for ApplicationError {
  fn from(error: StorageError) -> Self {
    match error {
      StorageError::NotFound(_) => Self::not_found(),
      StorageError::AlreadyExists(_) | StorageError::AlreadyOpen(_) => {
        Self::conflict(error.to_string())
      }
      StorageError::InvalidRange => Self::invalid(error.to_string()),
      _ => Self::internal(error),
    }
  }
}
impl From<rusqlite::Error> for ApplicationError {
  fn from(error: rusqlite::Error) -> Self {
    Self::internal(error)
  }
}
impl From<crate::tool::ask_user::AnswerError> for ApplicationError {
  fn from(error: crate::tool::ask_user::AnswerError) -> Self {
    use crate::tool::ask_user::AnswerError;
    match error {
      AnswerError::NotFound => Self::not_found(),
      AnswerError::Invalid(_) => Self::invalid(error.to_string()),
      AnswerError::Closed(_) => Self::conflict(error.to_string()),
    }
  }
}
impl From<crate::protocol::error::Error> for ApplicationError {
  fn from(error: crate::protocol::error::Error) -> Self {
    let kind = match &error {
      crate::protocol::error::Error::Build(_) => ErrorKind::InvalidInput,
      crate::protocol::error::Error::Unsupported { .. } => ErrorKind::Unsupported,
      _ => ErrorKind::Upstream,
    };
    Self { kind, message: error.to_string(), details: Some(json!(error)) }
  }
}

/// Admits storage work asynchronously, then runs it on the shared blocking pool.
///
/// Application use cases read and write the databases - `wish.sqlite` through a session or the
/// storage, `management.sqlite` through the index - inside `blocking`. The one exception is saving
/// a session's index record where the session changes (`SessionSlot::persist_index`, and the model
/// calls saved beside it when an operation ends): a single small write, which the engine's event
/// callbacks make too, and they cannot await.
pub async fn blocking<T: Send + 'static>(
  f: impl FnOnce() -> Result<T, ApplicationError> + Send + 'static,
) -> Result<T, ApplicationError> {
  crate::utils::blocking::io(f).await.map_err(ApplicationError::internal)?
}

impl From<crate::storage::repository_error::RepositoryError> for ApplicationError {
  fn from(error: crate::storage::repository_error::RepositoryError) -> Self {
    use crate::storage::repository_error::RepositoryError;
    match error {
      RepositoryError::Invalid(message) => Self::invalid(message),
      RepositoryError::Missing => Self::not_found(),
      other => Self::internal(other),
    }
  }
}

impl From<crate::task::TaskError> for ApplicationError {
  fn from(error: crate::task::TaskError) -> Self {
    use crate::task::TaskError;
    match error {
      TaskError::InvalidArguments(message) => Self::invalid(message),
      TaskError::ExecutionNotFound(_) => {
        Self { kind: ErrorKind::NotFound, message: error.to_string(), details: None }
      }
      TaskError::Interrupted | TaskError::Closed => Self::conflict(error.to_string()),
      TaskError::Unknown(message) => Self::unknown(message),
      TaskError::Io(error) => Self::internal(error),
    }
  }
}
