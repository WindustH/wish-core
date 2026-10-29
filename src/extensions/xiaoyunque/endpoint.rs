//! This supplier's authentication scheme, required headers and signing context.
use crate::protocol::{
  attempt::Call,
  endpoint::{AuthScheme, Credentials, Draft, Endpoint},
  error::Error,
};
pub fn ready(credentials: &Credentials) -> bool {
  crate::protocol::endpoint::volcengine::ready(credentials)
}
pub fn call(
  base: &str,
  path: &str,
  credentials: &Credentials,
  draft: Draft,
) -> Result<Call, Error> {
  let endpoint = Endpoint::new(base, path, AuthScheme::None)?;
  let mut call = endpoint.build_call(draft, credentials, crate::utils::time::unix_seconds())?;
  crate::protocol::endpoint::volcengine::sign(
    &mut call,
    credentials,
    "cn-north-1",
    "cv",
    crate::utils::time::unix_seconds(),
  )?;
  Ok(call)
}
