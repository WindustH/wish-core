//! Model providers: their configuration as the file holds it, and the client each one is built
//! into. A configuration save rebuilds them all.
use crate::server::{
  config::ProxyConfig, error::ApiError, management::ManagementStore,
  model_catalog::ModelCatalogCache, presets, sampling,
};
use crate::{
  client::{Client, CodeAgentIdentity},
  protocol::{
    TokenCountProtocol,
    account_state::AccountStateProtocol,
    endpoint::{
      AuthScheme, CredentialField, CredentialRenewal, Credentials, Endpoint, SourceHeader,
      copilot_oauth,
    },
    model_list::ModelListProtocol,
    model_use::{
      ModelUseProtocol,
      mode::{
        ChatCompletionApiCompatMode as Chat, MessagesApiCompatMode as Messages, ReasoningForm,
        ResponsesApiMode, ResponsesDeployment,
      },
    },
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
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.156.1";
/// How Wish names itself to a service that wants a client name.
pub const WISH_USER_AGENT: &str = concat!("wish/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
  pub display_name: Option<String>,
  pub preset: Option<String>,
  #[serde(default = "crate::server::config::yes")]
  pub enabled: bool,
  #[serde(default = "crate::server::config::yes")]
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
  /// Whether this is the Codex subscription preset, signed in through ChatGPT.
  pub fn is_codex(&self) -> bool {
    self.preset.as_deref() == Some("openai_codex")
  }
  /// Whether this is the GitHub Copilot subscription preset, signed in through GitHub.
  pub fn is_copilot(&self) -> bool {
    self.preset.as_deref() == Some("github_copilot")
  }
  /// How a subscription preset's bearer token renews; `None` for a key that never runs out.
  pub fn get_renewal(&self) -> Option<CredentialRenewal> {
    match self.preset.as_deref() {
      Some("openai_codex") => Some(CredentialRenewal::CodexOAuth),
      Some("github_copilot") => Some(CredentialRenewal::CopilotToken),
      _ => None,
    }
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
  /// service. Rebuilt providers start empty; see [`crate::server::model_catalog`].
  pub catalog: ModelCatalogCache,
}
impl Provider {
  pub fn build(id: String, config: ProviderConfig, proxy: &ProxyConfig) -> Result<Self, ApiError> {
    if let Some(preset) = &config.preset
      && presets::find_provider(preset).is_none()
    {
      return Err(ApiError::bad_request(format!("unknown provider preset: {preset}")));
    }
    let auth = match config.auth {
      Auth::None => AuthScheme::None,
      Auth::Bearer => AuthScheme::Bearer(config.get_renewal()),
      Auth::AnthropicKey => AuthScheme::Header("x-api-key"),
      Auth::GoogleKey => AuthScheme::Header("x-goog-api-key"),
      Auth::SigV4 => AuthScheme::SigV4,
    };
    let mut endpoint = Endpoint::new(&config.base_url, &config.path, auth)?;
    // Existing saved provider configs keep their own header maps when a preset is updated.
    // Apply current preset defaults here, then let explicit config headers override them.
    match config.preset.as_deref() {
      Some("opencode_go") => {
        endpoint = endpoint
          .with_header("x-opencode-session", "{session}")
          .with_header("user-agent", WISH_USER_AGENT);
      }
      Some("openai_codex") => {
        endpoint = endpoint
          .with_header("user-agent", CODEX_USER_AGENT)
          .with_header("session-id", "{session}")
          .with_header("thread-id", "{session}")
          .with_header("x-client-request-id", "{session}")
          .with_credential_header("chatgpt-account-id", CredentialField::AccountId);
      }
      Some("openai") => {
        endpoint = endpoint
          .with_header("user-agent", CODEX_USER_AGENT)
          .with_header("originator", "codex_cli_rs");
        if config.protocol == "openai_responses" {
          endpoint = endpoint
            .with_header("session-id", "{session}")
            .with_header("thread-id", "{session}")
            .with_header("x-client-request-id", "{session}");
        }
      }
      // Not Claude Code's own user agent: the API bills a call that names it as Claude Code
      // traffic, which an API key's credits do not pay for.
      Some("anthropic") => {
        endpoint = endpoint
          .with_header("user-agent", WISH_USER_AGENT)
          .with_header("x-app", "cli")
          .with_header("x-claude-code-session-id", "{session}");
      }
      Some("github_copilot") => {
        for header in copilot_oauth::API_HEADERS {
          if let SourceHeader::Literal(name, value) = header {
            endpoint = endpoint.with_header(name, value);
          }
        }
        endpoint = endpoint.with_header("openai-intent", "conversation-panel");
      }
      _ => {}
    }
    for (key, value) in &config.headers {
      endpoint = endpoint.with_header(key, value);
    }
    let mut credentials = Credentials::default();
    if config.enabled {
      credentials.api_key = config.api_key.clone().unwrap_or_default();
      credentials.refresh_token = config.refresh_token.clone();
      credentials.expires_at = config.expires_at;
      if let Some(name) = &config.api_key_env {
        credentials.api_key = read_secret(name).map_err(ApiError::bad_request)?;
      }
    }
    let mut values = config.credentials.clone();
    if config.enabled {
      for (field, env) in &config.credentials_env {
        values.insert(field.clone(), read_secret(env).map_err(ApiError::bad_request)?);
      }
    }
    for (field, value) in values {
      // The key has a setting of its own, so the map names only the fields beside it.
      match CredentialField::from_name(&field) {
        Some(known) if known != CredentialField::ApiKey => credentials.set_field(known, value),
        _ => return Err(ApiError::bad_request(format!("unknown credential: {field}"))),
      }
    }
    let mut client = Client::new(
      parse_protocol(&config.protocol)?,
      endpoint,
      ReqwestTransport::new(Default::default(), proxy.policy(config.proxy_enabled))?
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
        .map_err(|_| ApiError::bad_request("unknown token-count protocol"))?;
      client = client.with_token_count(protocol)?;
    }
    if let Some(value) = &config.compaction {
      let protocol = value
        .parse::<UpstreamCompactionProtocol>()
        .map_err(|_| ApiError::bad_request("unknown compaction protocol"))?;
      client = client.with_upstream_compaction(protocol)?;
    }
    if let Some(value) = &config.model_list {
      client = client.with_model_list(
        value.parse::<ModelListProtocol>().map_err(|e| ApiError::bad_request(e.to_string()))?,
      );
    }
    if let Some(value) = &config.account_state {
      client = client.with_account_state(
        value.parse::<AccountStateProtocol>().map_err(|e| ApiError::bad_request(e.to_string()))?,
      );
    }
    Ok(Self { id, client, config, catalog: Default::default() })
  }
  /// The client identity this provider's model calls carry, for a request made with its account
  /// outside a model call: a configured `user-agent`, else the one its preset sends.
  pub fn get_user_agent(&self) -> Option<String> {
    let configured =
      self.config.headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("user-agent"));
    if let Some((_, value)) = configured {
      return Some(value.clone());
    }
    match self.config.preset.as_deref() {
      Some("openai" | "openai_codex") => Some(CODEX_USER_AGENT.to_owned()),
      Some("anthropic" | "opencode_go") => Some(WISH_USER_AGENT.to_owned()),
      _ => None,
    }
  }
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
) -> Result<Providers, ApiError> {
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
/// A secret the configuration names by environment variable.
pub fn read_secret(name: &str) -> Result<String, String> {
  let value = std::env::var(name).map_err(|_| format!("environment variable {name} is missing"))?;
  if value.is_empty() {
    return Err(format!("environment variable {name} is empty"));
  }
  Ok(value)
}
fn parse_protocol(name: &str) -> Result<ModelUseProtocol, ApiError> {
  Ok(match name {
    "openai_chat" => ModelUseProtocol::OpenAiChat(Chat::Official),
    "compatible_chat" => ModelUseProtocol::OpenAiChat(Chat::Compatible),
    "deepseek_chat" => ModelUseProtocol::OpenAiChat(Chat::DeepSeek),
    "zai_chat" => ModelUseProtocol::OpenAiChat(Chat::Zai),
    "kimi_k2_chat" => ModelUseProtocol::OpenAiChat(Chat::KimiK2),
    "kimi_k3_chat" => ModelUseProtocol::OpenAiChat(Chat::KimiK3),
    "qwen_chat" => ModelUseProtocol::OpenAiChat(Chat::Qwen),
    "minimax_chat" => ModelUseProtocol::OpenAiChat(Chat::MiniMax),
    "mimo_chat" => ModelUseProtocol::OpenAiChat(Chat::Mimo),
    "tokenhub_chat" => ModelUseProtocol::OpenAiChat(Chat::TokenHub),
    "mistral_chat" => ModelUseProtocol::OpenAiChat(Chat::Mistral),
    "openai_responses" => ModelUseProtocol::OpenAiResponses(Default::default()),
    "plaintext_responses" => ModelUseProtocol::OpenAiResponses(ResponsesApiMode {
      reasoning_form: ReasoningForm::Plaintext,
      ..Default::default()
    }),
    "codex_responses" => ModelUseProtocol::OpenAiResponses(ResponsesApiMode {
      deployment: ResponsesDeployment::Codex,
      ..Default::default()
    }),
    "anthropic_messages" => ModelUseProtocol::AnthropicMessages(Messages::Official),
    "deepseek_messages" => ModelUseProtocol::AnthropicMessages(Messages::DeepSeek),
    "zai_messages" => ModelUseProtocol::AnthropicMessages(Messages::Zai),
    "kimi_messages" => ModelUseProtocol::AnthropicMessages(Messages::Kimi),
    "qwen_messages" => ModelUseProtocol::AnthropicMessages(Messages::Qwen),
    "minimax_messages" => ModelUseProtocol::AnthropicMessages(Messages::MiniMax),
    "mimo_messages" => ModelUseProtocol::AnthropicMessages(Messages::Mimo),
    "tokenhub_messages" => ModelUseProtocol::AnthropicMessages(Messages::TokenHub),
    "google_generate_content" => ModelUseProtocol::GoogleGenerateContent,
    "google_vertex_generate_content" => ModelUseProtocol::GoogleVertexGenerateContent,
    "google_interactions" => ModelUseProtocol::GoogleInteractions,
    "bedrock_converse" => ModelUseProtocol::BedrockConverse,
    "mistral_conversations" => ModelUseProtocol::MistralConversations,
    _ => return Err(ApiError::bad_request(format!("unknown model-use protocol: {name}"))),
  })
}
