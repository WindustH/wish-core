//! Independent webpage-reading services, sharing account credentials with other services.
use super::{
  accounts::AccountConfig,
  accounts::{AccountRegistry, AccountRuntime},
  config::ProxyConfig,
  error::ApplicationError,
};
use crate::{
  client::{RetryObserver, account::AccountClient},
  protocol::{
    attempt::Limits,
    endpoint::{AuthScheme, Endpoint},
    web_fetch::{FetchProtocol, FetchRequest},
  },
  transport::ReqwestTransport,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
  collections::BTreeMap,
  sync::{Arc, RwLock},
  time::Duration,
};
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FetchConfig {
  pub order: Vec<String>,
  pub providers: BTreeMap<String, FetchProvider>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FetchProvider {
  pub preset: String,
  #[serde(default)]
  pub credential_id: Option<String>,
  #[serde(default)]
  pub base_url: Option<String>,
  #[serde(default = "super::config::yes")]
  pub enabled: bool,
  #[serde(default = "super::config::yes")]
  pub proxy_enabled: bool,
  #[serde(default)]
  pub headers: BTreeMap<String, String>,
}
use crate::tool::web_fetch::FetchPage;
pub fn presets() -> Value {
  super::presets::fetch_presets().clone()
}
fn preset(id: &str) -> Result<&'static Value, ApplicationError> {
  super::presets::find_fetch(id)
    .ok_or_else(|| ApplicationError::invalid(format!("unknown fetch preset `{id}`")))
}
impl FetchConfig {
  pub fn validate(
    &self,
    accounts: &BTreeMap<String, AccountConfig>,
  ) -> Result<(), ApplicationError> {
    let mut seen = std::collections::HashSet::new();
    for id in &self.order {
      if !self.providers.contains_key(id) || !seen.insert(id) {
        return Err(ApplicationError::invalid("invalid fetch provider order"));
      }
    }
    for p in self.providers.values() {
      let spec = preset(&p.preset)?;
      let account = p
        .credential_id
        .as_ref()
        .map(|id| {
          accounts.get(id).ok_or_else(|| ApplicationError::invalid(format!("unknown account {id}")))
        })
        .transpose()?;
      if spec["key"] == "required" && account.is_none() {
        return Err(ApplicationError::invalid("fetch provider requires an account"));
      }
      if let Some(required) = spec["account_preset"].as_str()
        && account.and_then(|a| a.preset.as_deref()) != Some(required)
      {
        return Err(ApplicationError::invalid("incompatible fetch account"));
      }
      Endpoint::new(
        p.base_url.as_deref().unwrap_or(spec["base_url"].as_str().unwrap()),
        spec["path"].as_str().unwrap(),
        AuthScheme::None,
      )?;
    }
    Ok(())
  }
}
struct BuiltProvider {
  config: FetchProvider,
  protocol: FetchProtocol,
  requires_key: bool,
  transport: ReqwestTransport,
}
struct State {
  order: Vec<String>,
  providers: BTreeMap<String, BuiltProvider>,
}
/// Built before persisting configuration; applying it cannot fail midway through a save.
pub struct PreparedFetch(State);
pub struct FetchHub {
  state: RwLock<Arc<State>>,
}
impl FetchHub {
  pub fn new(config: FetchConfig, proxy: ProxyConfig) -> Result<Self, ApplicationError> {
    Ok(Self { state: RwLock::new(Arc::new(Self::prepare(&config, &proxy)?.0)) })
  }
  pub fn prepare(
    config: &FetchConfig,
    proxy: &ProxyConfig,
  ) -> Result<PreparedFetch, ApplicationError> {
    let mut providers = BTreeMap::new();
    for (id, config) in &config.providers {
      let spec = preset(&config.preset)?;
      let transport = ReqwestTransport::new(
        Limits {
          max_response_bytes: 8 * 1024 * 1024,
          total: Duration::from_secs(60),
          first_byte: Duration::from_secs(30),
          ..Default::default()
        },
        proxy.policy(config.proxy_enabled),
      )?;
      providers.insert(
        id.clone(),
        BuiltProvider {
          config: config.clone(),
          protocol: spec["protocol"].as_str().unwrap().parse()?,
          requires_key: spec["key"] == "required",
          transport,
        },
      );
    }
    Ok(PreparedFetch(State { order: config.order.clone(), providers }))
  }
  pub fn apply(&self, prepared: PreparedFetch) {
    *self.state.write().unwrap() = Arc::new(prepared.0);
  }
  pub fn available(&self) -> bool {
    let state = self.state.read().unwrap();
    state.order.iter().any(|id| state.providers.get(id).is_some_and(|p| p.config.enabled))
  }
  pub fn describe(&self) -> Value {
    let state = self.state.read().unwrap();
    json!({"providers":state.providers.iter().map(|(id,b)| {let p = &b.config; json!({"id":id,"preset":p.preset,"enabled":p.enabled,"credential_id":p.credential_id})}).collect::<Vec<_>>(),"available":state.order.iter().any(|id|state.providers.get(id).is_some_and(|p|p.config.enabled))})
  }
  pub async fn fetch(
    &self,
    url: &str,
    refresh: bool,
    conversation: &str,
    accounts: &AccountRegistry,
    observer: Option<RetryObserver>,
    only: Option<&str>,
  ) -> Result<FetchPage, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "invalid URL")?;
    if !matches!(parsed.scheme(), "http" | "https")
      || parsed.host_str().is_none()
      || !parsed.username().is_empty()
      || parsed.password().is_some()
    {
      return Err("URL must use HTTP/HTTPS without embedded credentials".into());
    }
    let state = self.state.read().unwrap().clone();
    let order = only.map(|id| vec![id.to_owned()]).unwrap_or_else(|| state.order.clone());
    let mut failures = Vec::new();
    for id in order {
      let Some(service) = state.providers.get(&id).filter(|p| p.config.enabled) else { continue };
      let account =
        service.config.credential_id.as_ref().and_then(|id| accounts.get(&format!("account:{id}")));
      match read(service, account, url, refresh, conversation, observer.clone()).await {
        Ok((title, content)) if !content.trim().is_empty() => {
          return Ok(FetchPage {
            url: url.into(),
            title,
            content,
            provider: id,
            fetched_at: crate::utils::time::unix_seconds(),
          });
        }
        Ok(_) => failures.push(format!("{id}: empty page")),
        Err(error) => failures.push(format!("{id}: {error}")),
      }
    }
    Err(if failures.is_empty() {
      "no enabled web fetch provider".into()
    } else {
      failures.join("; ")
    })
  }
}
async fn read(
  service: &BuiltProvider,
  account: Option<&Arc<AccountRuntime>>,
  url: &str,
  refresh: bool,
  conversation: &str,
  observer: Option<RetryObserver>,
) -> Result<(String, String), crate::protocol::error::Error> {
  let p = &service.config;
  let credentials = account.map(|a| a.client.get_credentials().clone()).unwrap_or_default();
  if service.requires_key && credentials.api_key.is_empty() {
    return Err(crate::protocol::error::Error::Build(
      "fetch account has no API key or active login".into(),
    ));
  }
  let protocol = service.protocol;
  let mut headers: Vec<(String, String)> =
    p.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
  let user_agent = account.and_then(|a| a.get_user_agent());
  headers.push((
    "user-agent".into(),
    user_agent.as_deref().unwrap_or(super::identity::WISH_USER_AGENT).into(),
  ));
  let request =
    FetchRequest { url: url.into(), refresh, conversation: Some(conversation.into()), model: None };
  let transport = &service.transport;
  // Shared-account calls use the same client boundary as subscription web search.
  if let Some(account) = account {
    let mut client = account.client.clone().with_transport(transport.clone());
    if let Some(observer) = observer {
      client = client.with_retry_observer(observer);
    }
    let page = client.fetch(protocol, &request, p.base_url.as_deref(), &headers).await?;
    return Ok((page.title, page.content));
  }
  let mut client = AccountClient::new(transport.clone(), credentials, None);
  if let Some(observer) = observer {
    client = client.with_retry_observer(observer);
  }
  let page = client.fetch(protocol, &request, p.base_url.as_deref(), &headers).await?;
  Ok((page.title, page.content))
}
