//! The configuration file's schema, and the checks a configuration passes at startup and on every
//! save before anything is built from it.
use crate::server::{
  error::ApiError, mcp::McpConfig, provider::ProviderConfig, search::SearchConfig,
  session::ToolSwitches,
};
use crate::session::{CompactionConfig, SessionConfig};
use crate::tool::shell::{self, ShellCommand};
use crate::transport::Proxy;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
  pub listen: SocketAddr,
  pub data_dir: PathBuf,
  pub bearer_token_env: Option<String>,
  /// The built web app to hand out beside the API; `web` beside the program when absent.
  /// Read at startup.
  pub web_dir: Option<PathBuf>,
  /// `host[:port]` names, besides loopback and the listen address, that browsers may reach a
  /// server without a token by: a LAN address or a domain. Read at startup.
  pub allowed_hosts: Vec<String>,
  pub providers: BTreeMap<String, ProviderConfig>,
  pub proxy: ProxyConfig,
  pub shell: ShellSettings,
  pub mcp: McpConfig,
  /// The services `web_search` asks, and in what order.
  pub search: SearchConfig,
  /// Where `wish skill` finds skills besides Wish's own directory, and which are off.
  pub skills: SkillsConfig,
  /// How much of the usage records is kept.
  pub usage: UsageConfig,
  pub defaults: Defaults,
}
impl Config {
  /// Refuses a proxy, MCP server, search provider or usage limit that could never work.
  pub fn validate(&self) -> Result<(), ApiError> {
    self.proxy.validate()?;
    self.mcp.validate(&self.providers)?;
    self.search.validate(&self.providers)?;
    self.usage.validate()
  }
}
/// The default of a switch that is on unless the file says otherwise.
pub(super) fn yes() -> bool {
  true
}

/// Skills a session's model finds with `wish skill`: Wish's own directory, `skills` beside the
/// configuration file, then these directories in order, after the session's `.agents/skills`.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillsConfig {
  /// More directories to find skills in; a leading `~` is the home directory.
  pub dirs: Vec<String>,
  /// Skills switched off, by name, wherever they are found.
  pub disabled: Vec<String>,
}

/// The usage records' size. Applied at once on a save.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct UsageConfig {
  /// The most one-second stream samples kept. Past it neighbouring samples are merged, those
  /// that would span the least time first, each session's and model's output and streaming time
  /// staying as they were, until it is met; `null` keeps every sample.
  pub stream_sample_limit: Option<u64>,
}
impl Default for UsageConfig {
  fn default() -> Self {
    Self { stream_sample_limit: Some(20_000) }
  }
}
impl UsageConfig {
  /// The fewest samples a limit may keep.
  const LEAST_SAMPLES: u64 = 100;
  fn validate(&self) -> Result<(), ApiError> {
    match self.stream_sample_limit {
      Some(limit) if limit < Self::LEAST_SAMPLES => Err(ApiError::bad_request(format!(
        "usage.stream_sample_limit must be at least {}",
        Self::LEAST_SAMPLES
      ))),
      _ => Ok(()),
    }
  }
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
  /// The tools new sessions start with.
  pub tools: ToolSwitches,
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
      tools: ToolSwitches {
        shell: true,
        ask_user: true,
        mcp: true,
        web_search: true,
        skills: true,
        sessions: true,
      },
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
      web_dir: None,
      allowed_hosts: Vec::new(),
      providers: BTreeMap::new(),
      proxy: ProxyConfig::default(),
      shell: ShellSettings::default(),
      mcp: Default::default(),
      search: Default::default(),
      skills: SkillsConfig::default(),
      usage: UsageConfig::default(),
      defaults: Defaults::default(),
    }
  }
}
