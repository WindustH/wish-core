//! Persistence errors independent of HTTP and application orchestration.
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
  #[error("{0}")]
  Invalid(String),
  #[error("not found")]
  Missing,
  #[error("{0}")]
  Internal(String),
}
impl RepositoryError {
  pub fn invalid(message: impl Into<String>) -> Self {
    Self::Invalid(message.into())
  }
  pub fn not_found() -> Self {
    Self::Missing
  }

  pub fn internal(error: impl std::fmt::Display) -> Self {
    Self::Internal(error.to_string())
  }
}
impl From<rusqlite::Error> for RepositoryError {
  fn from(error: rusqlite::Error) -> Self {
    Self::internal(error)
  }
}

impl From<super::StorageError> for RepositoryError {
  fn from(error: super::StorageError) -> Self {
    Self::internal(error)
  }
}
