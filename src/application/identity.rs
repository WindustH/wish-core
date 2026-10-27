//! Application identity and environment-backed configuration secrets.
pub const WISH_USER_AGENT: &str = concat!("wish/", env!("CARGO_PKG_VERSION"));
/// A secret the configuration names by environment variable.
pub fn read_secret(name: &str) -> Result<String, String> {
  let value = std::env::var(name).map_err(|_| format!("environment variable {name} is missing"))?;
  if value.is_empty() {
    return Err(format!("environment variable {name} is empty"));
  }
  Ok(value)
}
