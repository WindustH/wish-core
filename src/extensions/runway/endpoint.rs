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
  let endpoint = endpoint.with_header("X-Runway-Version", "2024-11-06");
  endpoint.build_call(draft, credentials, crate::utils::time::unix_seconds())
}
