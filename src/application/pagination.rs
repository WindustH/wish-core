//! Bounds for user-requested pages; storage only receives validated requests.
use super::error::ApplicationError;
pub fn validate_limit(limit: usize, maximum: usize) -> Result<(), ApplicationError> {
  if !(1..=maximum).contains(&limit) {
    return Err(ApplicationError::invalid(format!("limit must be between 1 and {maximum}")));
  }
  Ok(())
}
