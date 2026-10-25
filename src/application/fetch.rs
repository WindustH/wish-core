//! Independent webpage-reading services, sharing account credentials with other services.
use super::{
  accounts::AccountConfig,
  accounts::{AccountRegistry, AccountRuntime},
  config::ProxyConfig,
  error::ApplicationError,
};
use crate::{
  client::{
    RetryObserver, RetryOperation,
    retry::{classify_read_retry, retry_upstream},
  },
  protocol::{
    attempt::Limits,
    endpoint::{AuthScheme, Endpoint},
    web_fetch::{FetchProtocol, FetchRequest},
  },
  transport::ReqwestTransport,
  utils::retry::RetryPolicy,
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
#[derive(Clone, Deserialize, Serialize)]
pub struct Page {
  pub url: String,
  pub title: String,
  pub content: String,
  pub provider: String,
  pub fetched_at: u64,
}
pub fn presets() -> Value {
  serde_json::from_str(include_str!("../../resources/fetch-presets.json"))
    .expect("embedded fetch presets must be valid JSON")
}
fn preset(id: &str) -> Result<Value, ApplicationError> {
  presets()["presets"]
    .as_array()
    .unwrap()
    .iter()
    .find(|p| p["id"] == id)
    .cloned()
    .ok_or_else(|| ApplicationError::invalid("unknown fetch preset"))
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
      if let Some(required) = spec["account_preset"].as_str() {
        if account.and_then(|a| a.preset.as_deref()) != Some(required) {
          return Err(ApplicationError::invalid("incompatible fetch account"));
        }
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
pub struct FetchHub {
  state: RwLock<(FetchConfig, ProxyConfig)>,
}
impl FetchHub {
  pub fn new(config: FetchConfig, proxy: ProxyConfig) -> Self {
    Self { state: RwLock::new((config, proxy)) }
  }
  pub fn apply(&self, config: FetchConfig, proxy: ProxyConfig) {
    *self.state.write().unwrap() = (config, proxy);
  }
  pub fn available(&self) -> bool {
    let state = self.state.read().unwrap();
    state.0.order.iter().any(|id| state.0.providers.get(id).is_some_and(|p| p.enabled))
  }
  pub fn describe(&self) -> Value {
    let state = self.state.read().unwrap();
    json!({"providers":state.0.providers.iter().map(|(id,p)|json!({"id":id,"preset":p.preset,"enabled":p.enabled,"credential_id":p.credential_id})).collect::<Vec<_>>(),"available":state.0.order.iter().any(|id|state.0.providers.get(id).is_some_and(|p|p.enabled))})
  }
  pub async fn fetch(
    &self,
    url: &str,
    refresh: bool,
    conversation: &str,
    accounts: &AccountRegistry,
    observer: Option<RetryObserver>,
    only: Option<&str>,
  ) -> Result<Page, String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "invalid URL")?;
    if !matches!(parsed.scheme(), "http" | "https")
      || parsed.host_str().is_none()
      || !parsed.username().is_empty()
      || parsed.password().is_some()
    {
      return Err("URL must use HTTP/HTTPS without embedded credentials".into());
    }
    let (config, proxy) = self.state.read().unwrap().clone();
    let order = only.map(|id| vec![id.to_owned()]).unwrap_or(config.order);
    let mut failures = Vec::new();
    for id in order {
      let Some(service) = config.providers.get(&id).filter(|p| p.enabled) else { continue };
      let account =
        service.credential_id.as_ref().and_then(|id| accounts.get(&format!("account:{id}")));
      match read(service, account, url, refresh, conversation, &proxy, observer.clone()).await {
        Ok((title, content)) if !content.trim().is_empty() => {
          return Ok(Page {
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
  p: &FetchProvider,
  account: Option<&Arc<AccountRuntime>>,
  url: &str,
  refresh: bool,
  conversation: &str,
  proxy: &ProxyConfig,
  observer: Option<RetryObserver>,
) -> Result<(String, String), crate::Error> {
  let spec = preset(&p.preset).map_err(|_| crate::Error::Build("unknown fetch preset".into()))?;
  let credentials = account.map(|a| a.client.get_credentials().clone()).unwrap_or_default();
  if spec["key"] == "required" && credentials.api_key.is_empty() {
    return Err(crate::Error::Build("fetch account has no API key or active login".into()));
  }
  let protocol: FetchProtocol = spec["protocol"].as_str().unwrap().parse()?;
  let mut headers: Vec<(String, String)> =
    p.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
  let user_agent = account.and_then(|a| a.get_user_agent());
  headers.push((
    "user-agent".into(),
    user_agent.as_deref().unwrap_or(super::provider::WISH_USER_AGENT).into(),
  ));
  let request =
    FetchRequest { url: url.into(), refresh, conversation: Some(conversation.into()), model: None };
  let transport = ReqwestTransport::new(
    Limits {
      max_response_bytes: 8 * 1024 * 1024,
      total: Duration::from_secs(60),
      first_byte: Duration::from_secs(30),
      ..Default::default()
    },
    proxy.policy(p.proxy_enabled),
  )?;
  // Shared-account calls use the same client boundary as subscription web search.
  if let Some(account) = account {
    let mut client = account.client.clone().with_transport(transport);
    if let Some(observer) = observer {
      client = client.with_retry_observer(observer);
    }
    let page = client.fetch(protocol, &request, p.base_url.as_deref(), &headers).await?;
    return Ok((page.title, page.content));
  }
  let page = retry_upstream(
    &RetryPolicy::auxiliary(),
    RetryOperation::WebFetch,
    observer,
    |_| async {
      crate::protocol::web_fetch::fetch(
        &transport,
        protocol,
        &credentials,
        p.base_url.as_deref(),
        &headers,
        &request,
        crate::utils::time::unix_seconds(),
      )
      .await
    },
    classify_read_retry,
  )
  .await?;
  Ok((page.title, page.content))
}
