//! Opt-in transcription port; no closed supplier sum or common tool schema.
use crate::protocol::{
  attempt::Call,
  endpoint::{Credentials, Draft},
  error::Error,
};
pub trait Transcription: Send + Sync {
  fn registration(&self) -> &'static crate::extensions::catalog::Registration;
  fn credentials_ready(&self, c: &Credentials) -> bool;
  fn call(&self, base: &str, path: &str, c: &Credentials, d: Draft) -> Result<Call, Error>;
  fn result(&self, bytes: &[u8]) -> Result<serde_json::Value, String>;
  fn blob_id(&self) -> &str;
  fn service(&self) -> Option<&str>;
  fn validate_audio(&self, byte_len: u64, mime: &str) -> Result<(), Error>;
  fn submission(&self, model: &str, bytes: &[u8], mime: &str) -> Result<(String, Draft), Error>;
}
