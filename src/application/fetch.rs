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
  sync::{Arc, LazyLock, RwLock},
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
  static PRESETS: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../resources/fetch-presets.json"))
      .expect("embedded fetch presets must be valid JSON")
  });
  PRESETS.clone()
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
  ) -> Result<Page, String> {
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
  service: &BuiltProvider,
  account: Option<&Arc<AccountRuntime>>,
  url: &str,
  refresh: bool,
  conversation: &str,
  observer: Option<RetryObserver>,
) -> Result<(String, String), crate::Error> {
  let p = &service.config;
  let credentials = account.map(|a| a.client.get_credentials().clone()).unwrap_or_default();
  if service.requires_key && credentials.api_key.is_empty() {
    return Err(crate::Error::Build("fetch account has no API key or active login".into()));
  }
  let protocol = service.protocol;
  let mut headers: Vec<(String, String)> =
    p.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
  let user_agent = account.and_then(|a| a.get_user_agent());
  headers.push((
    "user-agent".into(),
    user_agent.as_deref().unwrap_or(super::provider::WISH_USER_AGENT).into(),
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
  let page = retry_upstream(
    &RetryPolicy::auxiliary(),
    RetryOperation::WebFetch,
    observer,
    |_| async {
      crate::protocol::web_fetch::fetch(
        transport,
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

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::{AtomicUsize, Ordering};
  use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

  #[tokio::test]
  async fn requests_reuse_connections_and_configuration_replaces_the_pool() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = connections.clone();
    let server = tokio::spawn(async move {
      let mut tasks = tokio::task::JoinSet::new();
      loop {
        let (socket, _) = listener.accept().await.unwrap();
        counter.fetch_add(1, Ordering::SeqCst);
        tasks.spawn(async move {
          let mut socket = BufReader::new(socket);
          loop {
            let mut length = 0;
            loop {
              let mut line = String::new();
              if socket.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
              }
              if line == "\r\n" {
                break;
              }
              if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse::<usize>().unwrap();
              }
            }
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            let page = r#"{"data":{"title":"Test","content":"Page content"}}"#;
            let response = format!(
              "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
              page.len(),
              page
            );
            if socket.get_mut().write_all(response.as_bytes()).await.is_err() {
              return;
            }
          }
        });
      }
    });
    let config = FetchConfig {
      order: vec!["reader".into()],
      providers: BTreeMap::from([(
        "reader".into(),
        FetchProvider {
          preset: "jina_reader".into(),
          credential_id: None,
          base_url: Some(format!("http://{address}")),
          enabled: true,
          proxy_enabled: false,
          headers: BTreeMap::new(),
        },
      )]),
    };
    let proxy = ProxyConfig::default();
    let hub = FetchHub::new(config.clone(), proxy.clone()).unwrap();
    let accounts = AccountRegistry::default();
    for _ in 0..5 {
      let page = tokio::time::timeout(
        Duration::from_secs(3),
        hub.fetch("https://example.com", false, "test", &accounts, None, None),
      )
      .await
      .unwrap()
      .unwrap();
      assert_eq!(page.content, "Page content");
    }
    assert_eq!(connections.load(Ordering::SeqCst), 1, "five calls share one TCP connection");
    // Existing requests may keep the previous snapshot; the next call uses the new runtime.
    let previous = hub.state.read().unwrap().clone();
    hub.apply(FetchHub::prepare(&config, &proxy).unwrap());
    assert!(!Arc::ptr_eq(&previous, &hub.state.read().unwrap()));
    hub.fetch("https://example.com", false, "test", &accounts, None, None).await.unwrap();
    assert_eq!(connections.load(Ordering::SeqCst), 2);
    server.abort();
    let _ = server.await;
  }
}
