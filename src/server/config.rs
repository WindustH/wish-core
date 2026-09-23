use crate::server::provider::ProviderConfig;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf};
use crate::session::{CompactionConfig, SessionConfig};

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
  pub listen: SocketAddr,
  pub data_dir: PathBuf,
  pub bearer_token_env: Option<String>,
  pub providers: BTreeMap<String, ProviderConfig>,
  pub defaults: Defaults,
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
