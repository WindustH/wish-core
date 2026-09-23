use crate::server::{config::read_secret, error::ApiError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use crate::{
  executor::model::Client,
  protocol::{
    TokenCountProtocol,
    account_state::AccountStateProtocol,
    model_list::ModelListProtocol,
    model_use::{
      ModelUseProtocol,
      request::{
        anthropic_messages::MessagesApiCompatMode as Messages,
        openai_chat::ChatCompletionApiCompatMode as Chat,
        openai_responses::{ReasoningForm, ResponsesApiCompatMode, ResponsesDeployment},
      },
    },
    outbound::{AuthProtocol, Credentials, Outbound},
    upstream_compaction::UpstreamCompactionProtocol,
  },
  transport::{Proxy, ReqwestTransport},
};

pub type ModelClient = Client<ReqwestTransport>;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
  pub display_name: Option<String>,
  pub preset: Option<String>,
  #[serde(default = "enabled")]
  pub enabled: bool,
  #[serde(default = "enabled")]
  pub proxy_enabled: bool,
  pub api_key: Option<String>,
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
}
fn enabled() -> bool {
  true
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
  pub client: ModelClient,
  pub config: ProviderConfig,
}
impl Provider {
  pub fn build(config: ProviderConfig) -> Result<Self, ApiError> {
    if let Some(id) = &config.preset {
      if crate::server::presets::find(id).is_none() {
        return Err(ApiError::bad_request(format!("unknown provider preset: {id}")));
      }
    }
    let auth = match config.auth {
      Auth::None => AuthProtocol::None,
      Auth::Bearer => AuthProtocol::Bearer(None),
      Auth::AnthropicKey => AuthProtocol::Header("x-api-key"),
      Auth::GoogleKey => AuthProtocol::Header("x-goog-api-key"),
      Auth::SigV4 => AuthProtocol::SigV4,
    };
    let mut outbound = Outbound::new(&config.base_url, &config.path, auth)?;
    for (key, value) in &config.headers {
      outbound = outbound.with_header(key, value);
    }
    let mut credentials = Credentials::default();
    if config.enabled {
      credentials.api_key = config.api_key.clone().unwrap_or_default();
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
      let value = Some(value);
      match field.as_str() {
        "region" => credentials.region = value,
        "access_key_id" => credentials.access_key_id = value,
        "secret_access_key" => credentials.secret_access_key = value,
        "session_token" => credentials.session_token = value,
        "account_id" => credentials.account_id = value,
        "workspace_id" => credentials.workspace_id = value,
        "team_id" => credentials.team_id = value,
        "organization" => credentials.organization = value,
        "project" => credentials.project = value,
        _ => return Err(ApiError::bad_request(format!("unknown credential: {field}"))),
      }
    }
    let mut client = Client::new(
      parse_protocol(&config.protocol)?,
      outbound,
      ReqwestTransport::new(
        Default::default(),
        if config.proxy_enabled { Proxy::Environment } else { Proxy::Disabled },
      )?,
    )
    .with_credentials(credentials);
    if let Some(value) = &config.token_count {
      client = client.with_token_count(match value.as_str() {
        "openai_responses" => TokenCountProtocol::OpenAiResponses,
        "anthropic_messages" => TokenCountProtocol::AnthropicMessages,
        "google_generate_content" => TokenCountProtocol::GoogleGenerateContent,
        _ => return Err(ApiError::bad_request("unknown token-count protocol")),
      })?;
    }
    if let Some(value) = &config.compaction {
      client = client.with_upstream_compaction(match value.as_str() {
        "openai_responses" => UpstreamCompactionProtocol::OpenAiResponses,
        "openai_responses_streamed" => UpstreamCompactionProtocol::OpenAiResponsesStreamed,
        _ => return Err(ApiError::bad_request("unknown compaction protocol")),
      })?;
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
    Ok(Self { client, config })
  }
  pub fn describe(&self, id: &str) -> serde_json::Value {
    // Static headers may contain credentials too. Never serialize provider config to HTTP.
    let preset = self.config.preset.as_deref().and_then(crate::server::presets::find);
    serde_json::json!({"id":id,"display_name":self.config.display_name,"preset":self.config.preset,"enabled":self.config.enabled,
      "brand":preset.map(|p|&p["provider"]),"reasoning_efforts":preset.map(|p|&p["reasoning_efforts"]),
      "max_output_tokens":preset.map(|p|&p["max_output_tokens"]),"protocol":self.config.protocol,"base_url":self.config.base_url,
      "token_count":self.config.token_count,"compaction":self.config.compaction,
      "model_list":self.config.model_list,"account_state":self.config.account_state,"models":self.config.models})
  }
}
fn parse_protocol(name: &str) -> Result<ModelUseProtocol, ApiError> {
  Ok(match name {
    "openai_chat" => ModelUseProtocol::OpenAiChat(Chat::Official),
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
    "plaintext_responses" => ModelUseProtocol::OpenAiResponses(ResponsesApiCompatMode {
      reasoning_form: ReasoningForm::Plaintext,
      ..Default::default()
    }),
    "codex_responses" => ModelUseProtocol::OpenAiResponses(ResponsesApiCompatMode {
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
