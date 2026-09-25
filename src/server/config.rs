use crate::server::error::ApiError;
use crate::server::provider::ProviderConfig;
use crate::session::{CompactionConfig, SessionConfig};
use crate::tool::shell::{self, ShellCommand};
use crate::transport::Proxy;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
  pub listen: SocketAddr,
  pub data_dir: PathBuf,
  pub bearer_token_env: Option<String>,
  pub providers: BTreeMap<String, ProviderConfig>,
  pub proxy: ProxyConfig,
  pub shell: ShellSettings,
  pub defaults: Defaults,
}

/// The shell every session's commands run under. Applied to the next command after a save.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellSettings {
  /// Absolute path of the shell; empty for the platform default.
  pub program: String,
  /// Arguments before the command text; absent for the ones the shell's family takes.
  pub args: Option<Vec<String>>,
}

impl ShellSettings {
  pub fn resolve(&self) -> Result<ShellCommand, ApiError> {
    let mut command = if self.program.is_empty() {
      ShellCommand::platform_default()
    } else {
      let program = PathBuf::from(&self.program);
      if !program.is_absolute() {
        return Err(ApiError::bad_request("shell program must be an absolute path"));
      }
      if !shell::is_runnable(&program) {
        return Err(ApiError::bad_request(format!(
          "shell program {} is not an executable file",
          program.display()
        )));
      }
      ShellCommand::for_program(program)
    };
    if let Some(args) = &self.args {
      command.args = args.clone();
    }
    Ok(command)
  }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProxyConfig {
  pub mode: ProxyMode,
  pub url: String,
  pub username: String,
  pub password: String,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
  #[default]
  Environment,
  Manual,
  Direct,
}

impl Default for ProxyConfig {
  fn default() -> Self {
    Self {
      mode: ProxyMode::Environment,
      url: String::new(),
      username: String::new(),
      password: String::new(),
    }
  }
}

impl ProxyConfig {
  pub fn validate(&self) -> Result<(), ApiError> {
    if self.url.is_empty() {
      return if matches!(self.mode, ProxyMode::Manual) {
        Err(ApiError::bad_request("manual proxy requires a URL"))
      } else {
        Ok(())
      };
    }
    let url = reqwest::Url::parse(&self.url)
      .map_err(|_| ApiError::bad_request("proxy URL must be valid"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
      return Err(ApiError::bad_request("manual proxy URL must use http:// or https://"));
    }
    if !url.username().is_empty() || url.password().is_some() {
      return Err(ApiError::bad_request(
        "put proxy credentials in the username and password fields, not the URL",
      ));
    }
    if self.username.is_empty() && !self.password.is_empty() {
      return Err(ApiError::bad_request("proxy username is required when password is set"));
    }
    Ok(())
  }

  pub fn policy(&self, provider_enabled: bool) -> Proxy {
    if !provider_enabled {
      return Proxy::Disabled;
    }
    match self.mode {
      ProxyMode::Environment => Proxy::Environment,
      ProxyMode::Direct => Proxy::Disabled,
      ProxyMode::Manual => Proxy::Manual {
        url: self.url.clone(),
        basic_auth: (!self.username.is_empty())
          .then(|| (self.username.clone(), self.password.clone())),
      },
    }
  }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
  pub provider: String,
  pub model: String,
  pub cwd: PathBuf,
  pub shell: bool,
  pub stream: bool,
  pub instructions: String,
  pub reasoning: Option<crate::protocol::ReasoningConfig>,
  pub max_output_tokens: Option<u64>,
  pub compaction: Option<CompactionConfig>,
}
impl Default for Defaults {
  fn default() -> Self {
    Self {
      provider: String::new(),
      model: String::new(),
      cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")),
      shell: true,
      stream: true,
      instructions: String::new(),
      reasoning: None,
      max_output_tokens: None,
      compaction: None,
    }
  }
}
impl Defaults {
  pub fn session_config(&self) -> SessionConfig {
    let mut config = SessionConfig::new(&self.model);
    config.stream = self.stream;
    config.reasoning = self.reasoning.clone();
    config.max_output_tokens = self.max_output_tokens;
    config.compaction = self.compaction.clone();
    config
  }
}
impl Default for Config {
  fn default() -> Self {
    Self {
      listen: "127.0.0.1:9780".parse().unwrap(),
      data_dir: PathBuf::from("data"),
      bearer_token_env: None,
      providers: BTreeMap::new(),
      proxy: ProxyConfig::default(),
      shell: ShellSettings::default(),
      defaults: Defaults::default(),
    }
  }
}
pub fn read_secret(name: &str) -> Result<String, String> {
  let value = std::env::var(name).map_err(|_| format!("environment variable {name} is missing"))?;
  if value.is_empty() {
    return Err(format!("environment variable {name} is empty"));
  }
  Ok(value)
}

/// The shell used when none is configured and the shells installed on this machine, each with
/// the arguments it would run with, for choosing one.
pub fn shell_catalog() -> Value {
  let describe = |command: ShellCommand| {
    json!({
      "name": command.program.file_stem().map(|name| name.to_string_lossy().into_owned()),
      "program": command.program,
      "args": command.args,
    })
  };
  json!({
    "default": describe(ShellCommand::platform_default()),
    "installed": ShellCommand::installed().into_iter().map(describe).collect::<Vec<_>>(),
  })
}

/// The proxy variables visible to this server process, with URL details that may carry secrets removed.
pub fn proxy_environment() -> Value {
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
  json!({"variables": variables})
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
