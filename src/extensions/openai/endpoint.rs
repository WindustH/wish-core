//! This supplier's authentication scheme, required headers and signing context.
use crate::protocol::{
  attempt::Call,
  endpoint::{AuthScheme, Credentials, Draft, Endpoint},
  error::Error,
};
pub fn ready(credentials: &Credentials) -> bool {
  !credentials.api_key.is_empty()
}
pub fn call(
  base: &str,
  path: &str,
  credentials: &Credentials,
  draft: Draft,
) -> Result<Call, Error> {
  let endpoint = Endpoint::new(base, path, AuthScheme::Bearer(None))?;
  endpoint.build_call(draft, credentials, crate::utils::time::unix_seconds())
}

pub fn subscription_image(
  base: &str,
  path: &str,
  credentials: &Credentials,
  draft: Draft,
) -> Result<Call, Error> {
  let mut endpoint = Endpoint::new(base, path, AuthScheme::Bearer(None))?
    .with_header("originator", "codex_cli_rs")
    .with_header("x-codex-image-turn-id", &uuid::Uuid::new_v4().to_string());
  if let Some(id) = &credentials.account_id {
    endpoint = endpoint.with_header("chatgpt-account-id", id);
  }
  endpoint.build_call(draft, credentials, crate::utils::time::unix_seconds())
}
