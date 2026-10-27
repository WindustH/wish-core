//! Model providers: their configuration as the file holds it, and the client each one is built
//! into. A configuration save rebuilds them all.
use crate::application::{
  config::ProxyConfig, error::ApplicationError, model_catalog::ModelCatalogCache, presets, sampling,
};
use crate::storage::management::ManagementStore;
use crate::{
  client::{Client, CodeAgentIdentity},
  protocol::{
    TokenCountProtocol,
    account_state::AccountStateProtocol,
    endpoint::{AuthScheme, CredentialField, CredentialRenewal, Credentials, Endpoint},
    model_list::ModelListProtocol,
    upstream_compaction::UpstreamCompactionProtocol,
  },
  transport::ReqwestTransport,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
use tokio_util::task::TaskTracker;

pub type ModelClient = Client<ReqwestTransport>;
/// The providers by id, as the application holds them.
pub type Providers = BTreeMap<String, Arc<Provider>>;

// Keep these defaults in step with the official stable CLI releases. Provider headers can
// override either value without changing the request-body identity fields.
/// How Wish names itself to a service that wants a client name.
use super::identity::{WISH_USER_AGENT, read_secret};
pub(crate) use crate::protocol::model_use::identity::CODEX_USER_AGENT;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
  pub display_name: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub credential_id: Option<String>,
  pub preset: Option<String>,
  #[serde(default = "crate::application::config::yes")]
  pub enabled: bool,
  #[serde(default = "crate::application::config::yes")]
  pub proxy_enabled: bool,
  pub api_key: Option<String>,
  #[serde(default)]
  pub refresh_token: Option<String>,
  #[serde(default)]
  pub expires_at: Option<u64>,
  #[serde(default)]
  pub credentials: BTreeMap<String, String>,
  pub model_list_base_url: Option<String>,
  #[serde(default)]
  pub models: BTreeMap<String, serde_json::Value>,
  pub protocol: String,
  pub base_url: String,
  pub path: String,
  #[serde(default)]
  pub auth: Auth,
  pub api_key_env: Option<String>,
  #[serde(default)]
  pub credentials_env: BTreeMap<String, String>,
  #[serde(default)]
  pub headers: BTreeMap<String, String>,
  pub token_count: Option<String>,
  pub compaction: Option<String>,
  pub model_list: Option<String>,
  pub model_list_path: Option<String>,
  pub account_state: Option<String>,
  /// Host the account reading is asked on, for a service that serves one account API from a
  /// regional twin (`open.bigmodel.cn` beside `api.z.ai`). Absent, the protocol's own host.
  pub account_state_base_url: Option<String>,
}
impl ProviderConfig {
  /// How a subscription preset's bearer token renews; `None` for a key that never runs out.
  pub fn get_renewal(&self) -> Option<CredentialRenewal> {
    super::presets::renewal(self.preset.as_deref())
  }
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
  None,
  #[default]
  Bearer,
  AnthropicKey,
  GoogleKey,
  SigV4,
}

pub struct Provider {
  /// The provider's key in the configuration.
  pub id: String,
  pub client: ModelClient,
  pub config: ProviderConfig,
  /// Pages read for `GET /providers/{id}/models`, so reopening a picker does not reread the
  /// service. Rebuilt providers start empty; see [`crate::application::model_catalog`].
  pub catalog: ModelCatalogCache,
}
impl Provider {
  pub fn build(
    id: String,
    config: ProviderConfig,
    proxy: &ProxyConfig,
  ) -> Result<Self, ApplicationError> {
    if let Some(preset) = &config.preset
      && presets::find_provider(preset).is_none()
    {
      return Err(ApplicationError::invalid(format!("unknown provider preset: {preset}")));
    }
    let auth = match config.auth {
      Auth::None => AuthScheme::None,
      Auth::Bearer => AuthScheme::Bearer(config.get_renewal()),
      Auth::AnthropicKey => AuthScheme::Header("x-api-key"),
      Auth::GoogleKey => AuthScheme::Header("x-goog-api-key"),
      Auth::SigV4 => AuthScheme::SigV4,
    };
    let mut endpoint = Endpoint::new(&config.base_url, &config.path, auth)?;
    endpoint = crate::protocol::model_use::identity::apply_headers(
      endpoint,
      config.preset.as_deref(),
      &config.protocol,
      WISH_USER_AGENT,
    );
    for (key, value) in &config.headers {
      endpoint = endpoint.with_header(key, value);
    }
    let mut credentials = Credentials::default();
    if config.enabled {
      credentials.api_key = config.api_key.clone().unwrap_or_default();
      credentials.refresh_token = config.refresh_token.clone();
      credentials.expires_at = config.expires_at;
      if let Some(name) = &config.api_key_env {
        credentials.api_key = read_secret(name).map_err(ApplicationError::invalid)?;
      }
    }
    let mut values = config.credentials.clone();
    if config.enabled {
      for (field, env) in &config.credentials_env {
        values.insert(field.clone(), read_secret(env).map_err(ApplicationError::invalid)?);
      }
    }
    for (field, value) in values {
      // The key has a setting of its own, so the map names only the fields beside it.
      match CredentialField::from_name(&field) {
        Some(known) if known != CredentialField::ApiKey => credentials.set_field(known, value),
        _ => return Err(ApplicationError::invalid(format!("unknown credential: {field}"))),
      }
    }
    let mut client = Client::new(
      crate::protocol::model_use::catalog::parse_protocol(&config.protocol)?,
      endpoint,
      ReqwestTransport::new(
        crate::protocol::attempt::Limits {
          // Models can think without sending body chunks. Give both body silence and buffered
          // attempts three times the transport defaults; keep connection/header limits intact.
          idle: std::time::Duration::from_secs(180),
          total: std::time::Duration::from_secs(900),
          ..Default::default()
        },
        proxy.policy(config.proxy_enabled),
      )?
      .with_stream_total(std::time::Duration::from_secs(30 * 60)),
    )
    .with_credentials(credentials);
    match config.preset.as_deref() {
      Some("openai" | "openai_codex") => {
        client = client.with_code_agent_identity(CodeAgentIdentity::Codex);
      }
      Some("anthropic") => {
        client = client.with_code_agent_identity(CodeAgentIdentity::Claude);
      }
      Some("github_copilot") => {
        client = client.with_code_agent_identity(CodeAgentIdentity::Copilot);
      }
      _ => {}
    }
    if let Some(value) = &config.token_count {
      let protocol = value
        .parse::<TokenCountProtocol>()
        .map_err(|_| ApplicationError::invalid("unknown token-count protocol"))?;
      client = client.with_token_count(protocol)?;
    }
    if let Some(value) = &config.compaction {
      let protocol = value
        .parse::<UpstreamCompactionProtocol>()
        .map_err(|_| ApplicationError::invalid("unknown compaction protocol"))?;
      client = client.with_upstream_compaction(protocol)?;
    }
    if let Some(value) = &config.model_list {
      client = client.with_model_list(
        value.parse::<ModelListProtocol>().map_err(|e| ApplicationError::invalid(e.to_string()))?,
      );
    }
    if let Some(value) = &config.account_state {
      client = client.with_account_state(
        value
          .parse::<AccountStateProtocol>()
          .map_err(|e| ApplicationError::invalid(e.to_string()))?,
      );
    }
    Ok(Self { id, client, config, catalog: Default::default() })
  }
  /// Public model-service metadata without credentials or secret headers.
  pub fn describe(&self) -> serde_json::Value {
    // Static headers may contain credentials too. Never serialize provider config to HTTP.
    let preset = self.config.preset.as_deref().and_then(presets::find_provider);
    serde_json::json!({"id":self.id,"display_name":self.config.display_name,"preset":self.config.preset,"enabled":self.config.enabled,
      "brand":preset.map(|p|&p["provider"]),"reasoning_efforts":preset.map(|p|&p["reasoning_efforts"]),
      "max_output_tokens":preset.map(|p|&p["max_output_tokens"]),"protocol":self.config.protocol,"base_url":self.config.base_url,
      "token_count":self.config.token_count,"compaction":self.config.compaction,
      "model_list":self.config.model_list,"account_state":self.config.account_state,"models":self.config.models})
  }
}
/// Builds every configured provider, each client sampling its streams into the index.
pub fn build_all(
  configs: &BTreeMap<String, ProviderConfig>,
  proxy: &ProxyConfig,
  management: &Arc<ManagementStore>,
  tasks: &TaskTracker,
) -> Result<Providers, ApplicationError> {
  let mut providers = BTreeMap::new();
  for (id, config) in configs {
    let mut provider = Provider::build(id.clone(), config.clone(), proxy)?;
    provider.client = sampling::with_stream_sampling(
      &provider.client,
      management.clone(),
      tasks.clone(),
      id.clone(),
      None,
    );
    providers.insert(id.clone(), Arc::new(provider));
  }
  Ok(providers)
}
