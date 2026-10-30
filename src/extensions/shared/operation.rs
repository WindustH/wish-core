//! A rejected operation is safe to reconsider; an unknown outcome may have external effects.
use serde_json::Value;
pub struct Report {
  pub text: String,
  pub output: Value,
  pub metadata: Value,
}
impl Report {
  pub fn json(output: Value) -> Self {
    Self { text: output.to_string(), metadata: output.clone(), output }
  }
}

#[derive(Debug)]
pub enum Failure {
  Rejected(String),
  Unknown(String),
}
impl From<String> for Failure {
  fn from(v: String) -> Self {
    Self::Rejected(v)
  }
}
impl From<&str> for Failure {
  fn from(v: &str) -> Self {
    Self::Rejected(v.into())
  }
}
