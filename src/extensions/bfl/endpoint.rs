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
  let endpoint = Endpoint::new(base, path, AuthScheme::Header("x-key"))?;
  endpoint.build_call(draft, credentials, crate::utils::time::unix_seconds())
}

/// BFL may return a regional task origin; only its HTTPS subdomains are trusted.
pub fn permits_poll_origin(origin: &reqwest::Url, target: &reqwest::Url) -> bool {
  origin.scheme() == "https"
    && target.scheme() == "https"
    && origin.host_str().is_some_and(|h| h == "api.bfl.ai" || h.ends_with(".bfl.ai"))
    && target.host_str().is_some_and(|h| h.ends_with(".bfl.ai"))
    && target.port_or_known_default() == Some(443)
}
