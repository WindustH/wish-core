//! Preserve whether a paid operation may already have produced an upstream side effect.
#[derive(Debug)]
pub enum Failure {
  Rejected(String),
  Unknown(String),
}
impl From<String> for Failure {
  fn from(value: String) -> Self {
    Self::Rejected(value)
  }
}
impl From<&str> for Failure {
  fn from(value: &str) -> Self {
    Self::Rejected(value.into())
  }
}
impl Failure {
  pub fn outcome(self) -> crate::session::ToolOutcome {
    match self {
      Self::Rejected(message) => crate::session::ToolOutcome::Failed(message),
      Self::Unknown(message) => crate::session::ToolOutcome::Unknown(format!(
        "{message}; upstream may have processed the request. Do not automatically resubmit."
      )),
    }
  }
}
