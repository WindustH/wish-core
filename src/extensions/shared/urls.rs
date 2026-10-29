//! Shared URL validation for service endpoints and artifact transfers.
use crate::protocol::{
  endpoint::{AuthScheme, Endpoint},
  error::Error,
};
pub fn validate_url(url: &str) -> Result<reqwest::Url, Error> {
  let parsed = reqwest::Url::parse(url).map_err(|_| Error::Build("invalid service URL".into()))?;
  if !matches!(parsed.scheme(), "https" | "http")
    || parsed.host_str().is_none()
    || !parsed.username().is_empty()
    || parsed.password().is_some()
  {
    return Err(Error::Build("URL must be HTTP(S) without credentials".into()));
  }
  Ok(parsed)
}
/// Service bases use Endpoint's stricter contract; artifact URLs may carry signed queries.
pub fn validate_base_url(url: &str) -> Result<(), Error> {
  Endpoint::new(url, "/", AuthScheme::None).map(|_| ())
}

/// Validate an input URL or an explicitly authorized supplier asset URI.
pub fn validate_asset_url(value: &str, asset_scheme: Option<&str>) -> Result<(), String> {
  if let Some(scheme) = asset_scheme
    && let Some(id) = value.strip_prefix(scheme)
  {
    if !id.is_empty()
      && id.len() <= 4096
      && !id.chars().any(|c| c.is_whitespace() || c.is_control())
    {
      return Ok(());
    }
    return Err("invalid provider asset URI".into());
  }
  let url = crate::extensions::shared::urls::validate_url(value).map_err(|e| e.to_string())?;
  if value.len() > 8192 || url.scheme() != "https" || url.fragment().is_some() {
    return Err("input URL must be public HTTPS without a fragment".into());
  }
  Ok(())
}
