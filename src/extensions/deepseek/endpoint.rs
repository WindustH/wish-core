//! DeepSeek tool APIs use the same API key as conversational models.
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
  Endpoint::new(base, path, AuthScheme::Bearer(None))?.build_call(
    draft,
    credentials,
    crate::utils::time::unix_seconds(),
  )
}
