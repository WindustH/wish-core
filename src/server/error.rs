//! Converts application failures to HTTP responses at the server boundary.
use crate::application::error::ErrorKind;
pub use crate::application::error::{ApplicationError as ApiError, blocking};
use axum::{
  Json,
  http::StatusCode,
  response::{IntoResponse, Response},
};
impl IntoResponse for ApiError {
  fn into_response(self) -> Response {
    let status = match self.kind {
      ErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
      ErrorKind::Conflict => StatusCode::CONFLICT,
      ErrorKind::NotFound => StatusCode::NOT_FOUND,
      ErrorKind::Unauthorized => StatusCode::UNAUTHORIZED,
      ErrorKind::Forbidden => StatusCode::FORBIDDEN,
      ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
      ErrorKind::Unsupported => StatusCode::NOT_IMPLEMENTED,
      ErrorKind::Upstream => StatusCode::BAD_GATEWAY,
      ErrorKind::Rejected => StatusCode::UNPROCESSABLE_ENTITY,
      ErrorKind::Timeout => StatusCode::GATEWAY_TIMEOUT,
    };
    (status, Json(serde_json::json!({"error":{"message":self.message,"details":self.details}})))
      .into_response()
  }
}
