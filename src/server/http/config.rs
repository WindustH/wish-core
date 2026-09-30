//! The settings page: the configuration file, redacted, and saving it; the defaults new sessions
//! start from; and what this machine offers to choose from - its shells and the proxy variables
//! the server runs with.
use crate::server::{app::App, error::ApiError};
use crate::tool::shell::ShellCommand;
use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub async fn get(State(app): State<Arc<App>>) -> Json<Value> {
  Json(app.config_file.lock().await.redacted())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveConfig {
  revision: String,
  config: Value,
}
pub async fn save(
  State(app): State<Arc<App>>,
  Json(input): Json<SaveConfig>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  Ok(Json(app.save_config(input.revision, input.config).await?))
}
pub async fn defaults(State(app): State<Arc<App>>) -> Json<Value> {
  let file = app.config_file.lock().await;
  let defaults = &file.config.defaults;
  Json(json!({"defaults":defaults,"session_config":defaults.session_config()}))
}
/// The shell used when none is configured and the shells installed on this machine, each with
/// the arguments it would run with, for choosing one.
pub async fn shells() -> Json<Value> {
  let describe = |command: ShellCommand| {
    json!({
      "name": command.program.file_stem().map(|name| name.to_string_lossy().into_owned()),
      "program": command.program,
      "args": command.args,
    })
  };
  Json(json!({
    "default": describe(ShellCommand::platform_default()),
    "installed": ShellCommand::installed().into_iter().map(describe).collect::<Vec<_>>(),
  }))
}

/// The proxy variables visible to this server process, with URL details that may carry secrets
/// removed.
pub async fn proxy_environment() -> Json<Value> {
  const NAMES: &[&str] = &[
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
  ];
  let variables: Vec<Value> = NAMES
    .iter()
    .filter_map(|name| {
      let raw = std::env::var_os(name)?;
      let value = raw.to_string_lossy();
      let (displayed, redacted) = if name.eq_ignore_ascii_case("no_proxy") {
        (value.into_owned(), false)
      } else {
        display_proxy_address(&value)
      };
      Some(json!({"name": name, "value": displayed, "redacted": redacted}))
    })
    .collect();
  Json(json!({"variables": variables}))
}

fn display_proxy_address(raw: &str) -> (String, bool) {
  let parsed = reqwest::Url::parse(raw)
    .ok()
    .filter(|url| url.host_str().is_some())
    .or_else(|| reqwest::Url::parse(&format!("http://{raw}")).ok());
  let Some(mut url) = parsed else {
    return ("—".to_owned(), true);
  };
  if url.host_str().is_none() {
    return ("—".to_owned(), true);
  }
  let hidden = !url.username().is_empty()
    || url.password().is_some()
    || url.path() != "/"
    || url.query().is_some()
    || url.fragment().is_some();
  let _ = url.set_username("");
  let _ = url.set_password(None);
  url.set_path("/");
  url.set_query(None);
  url.set_fragment(None);
  (url.to_string(), hidden)
}
