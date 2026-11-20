//! The configuration file's schema, and the checks a configuration passes at startup and on every
//! save before anything is built from it.
use crate::application::{
  error::ApplicationError, mcp::McpConfig, provider::ProviderConfig, search::SearchConfig,
  session::ToolSwitches,
};
use crate::session::{CompactionConfig, RunOptions, SessionConfig};
use crate::task::{self, ShellCommand};
use crate::transport::Proxy;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
  pub listen: SocketAddr,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub ssh: Option<SshSettings>,
  pub data_dir: PathBuf,
  pub bearer_token_env: Option<String>,
  /// The built web app to hand out beside the API; `web` beside the program when absent.
  /// Read at startup.
  pub web_dir: Option<PathBuf>,
  /// `host[:port]` names, besides loopback and the listen address, that browsers may reach a
  /// server without a token by: a LAN address or a domain. Read at startup.
  pub allowed_hosts: Vec<String>,
  pub providers: BTreeMap<String, ProviderConfig>,
  pub accounts: BTreeMap<String, super::accounts::AccountConfig>,
  pub proxy: ProxyConfig,
  pub shell: ShellSettings,
  pub terminal: task::TerminalSettings,
  pub mcp: McpConfig,
  /// The services `web_search` asks, and in what order.
  pub search: SearchConfig,
  pub fetch: super::fetch::FetchConfig,
  pub image_generation: super::image_generation::ImageConfig,
  pub video_generation: super::media_generation::MediaConfig,
  pub audio_generation: super::media_generation::MediaConfig,
  pub audio_transcription: super::media_generation::MediaConfig,
  /// Where `wish skill` finds skills besides Wish's own directory, and which are off.
  pub skills: SkillsConfig,
  /// How much of the usage records is kept.
  pub usage: UsageConfig,
  pub defaults: Defaults,
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SshSettings {
  pub listen: SocketAddr,
  pub host_key: Option<PathBuf>,
  pub authorized_keys: PathBuf,
  #[serde(default = "rekey_default")]
  pub rekey_seconds: u32,
}
fn rekey_default() -> u32 {
  3600
}

impl Config {
  /// Refuses a proxy, MCP server, search provider or usage limit that could never work.
  pub fn validate(&self) -> Result<(), ApplicationError> {
    if let Some(ssh) = &self.ssh {
      if !self.listen.ip().is_loopback() {
        return Err(ApplicationError::invalid(
          "ssh requires the HTTP listen address to be loopback",
        ));
      }
      if ssh.rekey_seconds == 0 {
        return Err(ApplicationError::invalid("ssh.rekey_seconds must be positive"));
      }
    }
    let terminal_shell = self.terminal.shell.trim();
    if !terminal_shell.is_empty()
      && (!std::path::Path::new(terminal_shell).is_absolute()
        || !crate::task::is_runnable(std::path::Path::new(terminal_shell)))
    {
      return Err(ApplicationError::invalid(
        "terminal.shell must be an absolute path to an executable",
      ));
    }
    if self.terminal.session_name.trim().starts_with('-')
      || matches!(self.terminal.session_name.trim(), "options" | "help")
    {
      return Err(ApplicationError::invalid(
        "terminal.session_name cannot start with '-' or be a multiplexer command name",
      ));
    }
    if self.terminal.session_name.len() > 128
      || self.terminal.session_name.chars().any(|c| c.is_control())
    {
      return Err(ApplicationError::invalid(
        "terminal.session_name must be at most 128 bytes without control characters",
      ));
    }
    if !(8..=32).contains(&self.terminal.font_size) {
      return Err(ApplicationError::invalid("terminal.font_size must be between 8 and 32"));
    }
    for (key, value) in &self.terminal.colors {
      if ![
        "background",
        "foreground",
        "cursor",
        "selectionBackground",
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "white",
        "brightBlack",
        "brightRed",
        "brightGreen",
        "brightYellow",
        "brightBlue",
        "brightMagenta",
        "brightCyan",
        "brightWhite",
      ]
      .contains(&key.as_str())
        || value.len() != 7
        || !value.starts_with('#')
        || !value[1..].bytes().all(|b| b.is_ascii_hexdigit())
      {
        return Err(ApplicationError::invalid(
          "terminal.colors must use supported color names and #RRGGBB values",
        ));
      }
    }
    self.proxy.validate()?;
    self.mcp.validate(&self.providers)?;
    let providers = self
      .accounts
      .iter()
      .map(|(id, account)| (format!("account:{id}"), account.clone()))
      .collect();
    self.resolved_search()?.validate(&providers)?;
    self.fetch.validate(&self.accounts)?;
    self.image_generation.validate(&self.accounts)?;
    self.video_generation.validate(&self.accounts, "video")?;
    self.audio_generation.validate(&self.accounts, "audio")?;
    self.audio_transcription.validate(&self.accounts, "transcription")?;
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
  fn validate(&self) -> Result<(), ApplicationError> {
    match self.stream_sample_limit {
      Some(limit) if limit < Self::LEAST_SAMPLES => Err(ApplicationError::invalid(format!(
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
  /// Overrides inherited variables for new commands. Values may contain credentials.
  #[serde(skip_serializing_if = "BTreeMap::is_empty")]
  pub env: BTreeMap<String, String>,
  /// Derive proxy variables from network settings before applying explicit `env` overrides.
  #[serde(skip_serializing_if = "is_false")]
  pub use_network_proxy: bool,
}

fn is_false(value: &bool) -> bool {
  !value
}

impl ShellSettings {
  pub fn resolve(&self, proxy: &ProxyConfig) -> Result<ShellCommand, ApplicationError> {
    let mut command = if self.program.is_empty() {
      ShellCommand::platform_default()
    } else {
      let program = PathBuf::from(&self.program);
      if !program.is_absolute() {
        return Err(ApplicationError::invalid("shell program must be an absolute path"));
      }
      if !task::is_runnable(&program) {
        return Err(ApplicationError::invalid(format!(
          "shell program {} is not an executable file",
          program.display()
        )));
      }
      ShellCommand::for_program(program)
    };
    if let Some(args) = &self.args {
      command.args = args.clone();
    }
    if self.use_network_proxy {
      const NAMES: &[&str] =
        &["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"];
      match proxy.mode {
        ProxyMode::Environment => {} // Inherit the server's actual environment, including NO_PROXY.
        ProxyMode::Direct => {
          command.env.extend(NAMES.iter().map(|name| ((*name).into(), None)));
        }
        ProxyMode::Manual => {
          proxy.validate()?;
          let mut url = reqwest::Url::parse(&proxy.url)
            .map_err(|_| ApplicationError::invalid("proxy URL must be valid"))?;
          if !proxy.username.is_empty() {
            url
              .set_username(&proxy.username)
              .map_err(|_| ApplicationError::invalid("proxy URL cannot contain credentials"))?;
            url
              .set_password(Some(&proxy.password))
              .map_err(|_| ApplicationError::invalid("proxy URL cannot contain credentials"))?;
          }
          command.env.extend(NAMES.iter().map(|name| ((*name).into(), Some(url.to_string()))));
        }
      }
    }
    for (name, value) in &self.env {
      if name.is_empty()
        || !name
          .bytes()
          .enumerate()
          .all(|(i, c)| c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
      {
        return Err(ApplicationError::invalid(
          "shell environment names must start with a letter or underscore and contain only letters, digits and underscores",
        ));
      }
      if name.to_ascii_uppercase().starts_with("WISH_") {
        return Err(ApplicationError::invalid(
          "shell environment names starting with WISH_ are reserved",
        ));
      }
      if value.contains('\0') {
        return Err(ApplicationError::invalid("shell environment values cannot contain NUL"));
      }
      // Windows treats names case-insensitively; a generated lowercase alias must not replace
      // an explicit uppercase override later while constructing the process environment.
      #[cfg(windows)]
      command.env.retain(|key, _| !key.eq_ignore_ascii_case(name));
      command.env.insert(name.clone(), Some(value.clone()));
    }
    Ok(command)
  }

  /// A session override can retain its own values, or copy the global settings on first use.
  pub fn restore_env(&mut self, stored: &Self) -> Result<(), ApplicationError> {
    for (name, value) in &mut self.env {
      if value == "<redacted>" {
        *value = stored.env.get(name).cloned().ok_or_else(|| {
          ApplicationError::invalid(format!(
            "redacted shell env value `{name}` has no stored value"
          ))
        })?;
      }
    }
    Ok(())
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
  pub fn validate(&self) -> Result<(), ApplicationError> {
    if self.url.is_empty() {
      return if matches!(self.mode, ProxyMode::Manual) {
        Err(ApplicationError::invalid("manual proxy requires a URL"))
      } else {
        Ok(())
      };
    }
    let url = reqwest::Url::parse(&self.url)
      .map_err(|_| ApplicationError::invalid("proxy URL must be valid"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
      return Err(ApplicationError::invalid("manual proxy URL must use http:// or https://"));
    }
    if !url.username().is_empty() || url.password().is_some() {
      return Err(ApplicationError::invalid(
        "put proxy credentials in the username and password fields, not the URL",
      ));
    }
    if self.username.is_empty() && !self.password.is_empty() {
      return Err(ApplicationError::invalid("proxy username is required when password is set"));
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
  /// The tools new sessions start with; stored sessions retain their own compatibility defaults.
  #[serde(deserialize_with = "deserialize_default_tools")]
  pub tools: ToolSwitches,
  pub stream: bool,
  /// Execution policy copied into new sessions.
  #[serde(deserialize_with = "deserialize_default_run")]
  pub run: RunOptions,
  pub instructions: String,
  pub reasoning: Option<crate::protocol::ReasoningConfig>,
  pub max_output_tokens: Option<u64>,
  pub compaction: Option<CompactionConfig>,
}
/// Missing global fields use today's defaults, independently of persisted session decoding.
fn deserialize_default_tools<'de, D: serde::Deserializer<'de>>(
  deserializer: D,
) -> Result<ToolSwitches, D::Error> {
  let changes = super::session::ToolChanges::deserialize(deserializer)?;
  Ok(Defaults::default().tools.apply_changes(changes))
}

fn deserialize_default_run<'de, D: serde::Deserializer<'de>>(
  deserializer: D,
) -> Result<RunOptions, D::Error> {
  #[derive(Deserialize)]
  #[serde(deny_unknown_fields)]
  struct Changes {
    tools: Option<crate::session::ToolMode>,
    background_tools_on_input: Option<bool>,
  }
  let changes = Changes::deserialize(deserializer)?;
  Ok(RunOptions {
    tools: changes.tools.unwrap_or_default(),
    background_tools_on_input: changes.background_tools_on_input.unwrap_or(true),
  })
}

impl Default for Defaults {
  fn default() -> Self {
    Self {
      provider: String::new(),
      model: String::new(),
      cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/tmp")),
      tools: ToolSwitches {
        blob: true,
        blob_delivery: super::session::ToolDelivery::Command,
        history_search: true,
        history_search_delivery: super::session::ToolDelivery::Command,
        task_delivery: super::session::ToolDelivery::Command,
        web_fetch_delivery: super::session::ToolDelivery::Command,
        web_search_delivery: super::session::ToolDelivery::Command,
        ask_user_delivery: super::session::ToolDelivery::Command,
        shell: true,
        ask_user: true,
        mcp: true,
        web_search: true,
        web_fetch: true,
        video_generate: false,
        video_generate_delivery: super::session::ToolDelivery::Tool,
        audio_transcribe: false,
        audio_transcribe_delivery: super::session::ToolDelivery::Tool,
        audio_generate: false,
        audio_generate_delivery: super::session::ToolDelivery::Tool,
        image_generate: false,
        image_generate_delivery: super::session::ToolDelivery::Tool,
        skills: true,
        sessions: true,
        sessions_delivery: super::session::ToolDelivery::Command,
      },
      stream: true,
      run: RunOptions { background_tools_on_input: true, ..Default::default() },
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
    config.run = self.run;
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
      ssh: None,
      providers: BTreeMap::new(),
      accounts: BTreeMap::new(),
      proxy: ProxyConfig::default(),
      shell: ShellSettings::default(),
      terminal: Default::default(),
      mcp: Default::default(),
      search: Default::default(),
      fetch: Default::default(),
      image_generation: Default::default(),
      video_generation: Default::default(),
      audio_generation: Default::default(),
      audio_transcription: Default::default(),
      skills: SkillsConfig::default(),
      usage: UsageConfig::default(),
      defaults: Defaults::default(),
    }
  }
}
