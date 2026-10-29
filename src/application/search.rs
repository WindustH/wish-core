//! Search providers: the services `web_search` asks, configured beside the model providers.
//!
//! A search provider is one of two kinds. One that comes with a subscription borrows a model
//! provider's account (`auth_provider`): its credentials, its proxy and its renewals, taken at the
//! moment of each search from the model provider as it then stands, so nothing here holds a copy
//! that could go stale. Any other brings a key of its own. `order` is the order they are asked in:
//! when one fails - its quota spent, its key refused, its service down - the next is asked, and a
//! search fails only when every one has.

use crate::application::{
  accounts::{AccountConfig, AccountRegistry},
  config::ProxyConfig,
  error::ApplicationError,
  identity::{WISH_USER_AGENT, read_secret},
  presets,
};
use crate::client::RetryObserver;
use crate::extension_host::search::{SearchProtocol, SearchQuery, SearchResults};
use crate::extensions::shared::read::ReadClient;
use crate::protocol::endpoint::Credentials;
use crate::transport::ReqwestTransport;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
  collections::BTreeMap,
  sync::{Arc, RwLock},
  time::{Duration, Instant},
};

/// How long each physical attempt has to answer; retries have their own waits.
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchConfig {
  /// The providers in the order they are asked. One left out is never asked.
  pub order: Vec<String>,
  pub providers: BTreeMap<String, SearchProviderConfig>,
}

/// One configured search provider.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SearchProviderConfig {
  /// The search preset, which names the protocol and the service's own address.
  pub preset: String,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub credential_id: Option<String>,
  #[serde(default = "crate::application::config::yes")]
  pub enabled: bool,
  /// Where the service is, for a self-hosted one or another region; absent, the preset's.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub base_url: Option<String>,
  /// The model provider whose account a subscription's search borrows.
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub auth_provider: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub api_key: Option<String>,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub api_key_env: Option<String>,
  /// Whether an own-key provider goes through the configured proxy. A borrowed one follows its
  /// model provider.
  #[serde(default = "crate::application::config::yes")]
  pub proxy_enabled: bool,
  #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
  pub headers: BTreeMap<String, String>,
}

impl SearchConfig {
  /// Refuses what could never search: an unknown preset, a borrowed account that does not exist
  /// or is not one the preset comes with, a key where the account is borrowed, or none where one
  /// is needed.
  pub fn validate(
    &self,
    accounts: &BTreeMap<String, AccountConfig>,
  ) -> Result<(), ApplicationError> {
    for id in &self.order {
      if !self.providers.contains_key(id) {
        return Err(ApplicationError::invalid(format!("search order names unknown provider {id}")));
      }
    }
    for (index, id) in self.order.iter().enumerate() {
      if self.order[..index].contains(id) {
        return Err(ApplicationError::invalid(format!("search order names {id} twice")));
      }
    }
    for (id, config) in &self.providers {
      let preset = find_preset(id, config)?;
      let lenders: Vec<&str> =
        preset["borrows_from"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
      match (&config.auth_provider, lenders.is_empty()) {
        (Some(lender), false) => {
          let account = accounts.get(lender).ok_or_else(|| {
            ApplicationError::invalid(format!(
              "search provider {id} borrows missing provider {lender}"
            ))
          })?;
          if !account.preset.as_deref().is_some_and(|preset| lenders.contains(&preset)) {
            return Err(ApplicationError::invalid(format!(
              "search provider {id} cannot borrow {lender}: {} comes only with {}",
              config.preset,
              lenders.join(", ")
            )));
          }
          if config.api_key.is_some() || config.api_key_env.is_some() {
            return Err(ApplicationError::invalid(format!(
              "search provider {id} borrows {lender}'s account and takes no key of its own"
            )));
          }
        }
        (Some(_), true) => {
          return Err(ApplicationError::invalid(format!(
            "search provider {id}: {} has no account to borrow",
            config.preset
          )));
        }
        (None, false) => {
          return Err(ApplicationError::invalid(format!(
            "search provider {id}: {} needs the model provider it comes with",
            config.preset
          )));
        }
        (None, true) => {
          if preset["key"] == "required" && config.api_key.is_none() && config.api_key_env.is_none()
          {
            return Err(ApplicationError::invalid(format!(
              "search provider {id} needs an API key"
            )));
          }
          if parse_protocol(preset)?.get_default_base_url().is_none()
            && config.base_url.as_deref().is_none_or(str::is_empty)
          {
            return Err(ApplicationError::invalid(format!(
              "search provider {id} needs the address of its service"
            )));
          }
        }
      }
      if let Some(url) = &config.base_url {
        crate::extensions::shared::urls::validate_base_url(url)?;
      }
    }
    Ok(())
  }
}

/// How a provider's searches are proven.
enum Account {
  /// A model provider's, looked up at each search.
  Borrowed(String, ReqwestTransport),
  /// Its own key, through a client of its own.
  Own(Box<OwnAccount>),
}

struct OwnAccount {
  transport: ReqwestTransport,
  credentials: Credentials,
}

struct Built {
  config: SearchProviderConfig,
  preset: &'static Value,
  protocol: SearchProtocol,
  account: Account,
}

#[derive(Default)]
struct State {
  order: Vec<String>,
  providers: BTreeMap<String, Built>,
}

use crate::tool::web_search::Answer;

/// The configured search providers, rebuilt whole on every configuration save.
pub struct SearchHub {
  state: RwLock<Arc<State>>,
}

impl SearchHub {
  pub fn new(config: &SearchConfig, proxy: &ProxyConfig) -> Result<Self, ApplicationError> {
    Ok(Self { state: RwLock::new(Arc::new(build(config, proxy)?)) })
  }

  /// Builds the next configuration's providers without putting them in place, so a save can be
  /// refused before anything is written.
  pub fn prepare(
    config: &SearchConfig,
    proxy: &ProxyConfig,
  ) -> Result<PreparedSearch, ApplicationError> {
    Ok(PreparedSearch(build(config, proxy)?))
  }

  pub fn apply(&self, prepared: PreparedSearch) {
    *self.state.write().unwrap() = Arc::new(prepared.0);
  }

  /// Whether some provider in the order could search now.
  pub fn is_available(&self, accounts: &AccountRegistry) -> bool {
    let state = self.get_state();
    state
      .order
      .iter()
      .any(|id| state.providers.get(id).is_some_and(|built| usable(built, accounts).is_ok()))
  }

  /// Every configured provider, in order first, with whether it can search and why not.
  pub fn describe(&self, accounts: &AccountRegistry) -> Vec<Value> {
    let state = self.get_state();
    let mut ids: Vec<&String> = state.order.iter().collect();
    ids.extend(state.providers.keys().filter(|id| !state.order.contains(id)));
    ids
      .into_iter()
      .filter_map(|id| {
        let built = state.providers.get(id)?;
        let problem = usable(built, accounts).err();
        Some(json!({
          "id": id,
          "preset": built.config.preset,
          "name": built.preset["name"],
          "protocol": built.protocol.get_id(),
          "base_url": built.config.base_url.as_deref().or(built.protocol.get_default_base_url()),
          "auth_provider": built.config.auth_provider,
          "enabled": built.config.enabled,
          "ordered": state.order.contains(id),
          "available": problem.is_none(),
          "problem": problem,
        }))
      })
      .collect()
  }

  pub fn provider_catalog(&self, accounts: &AccountRegistry) -> crate::tool::providers::Catalog {
    let state = self.get_state();
    crate::tool::providers::Catalog {
      default_order: state.order.clone(),
      failure_policy: "try_in_order_unless_provider_is_explicit",
      providers: state
        .providers
        .iter()
        .map(|(id, built)| crate::tool::providers::Provider {
          id: id.clone(),
          name: built.preset["name"].as_str().unwrap_or(id).into(),
          preset: built.config.preset.clone(),
          kind: None,
          model: built.preset["model"].as_str().map(str::to_owned),
          reference_images: None,
          mask_editing: None,
          markup_editing: None,
          image_required: None,
          image_options: None,
          precise_references: None,
          video: None,
          enabled: built.config.enabled,
          available: usable(built, accounts).is_ok(),
        })
        .collect(),
    }
  }

  /// Asks the providers in order until one answers. On failure, every provider's reason.
  pub async fn search(
    &self,
    accounts: &AccountRegistry,
    query: &SearchQuery,
    observer: Option<RetryObserver>,
    only: Option<&str>,
  ) -> Result<Answer, Vec<String>> {
    crate::extensions::shared::read_budget::run(self.search_inner(accounts, query, observer, only))
      .await
      .map_err(|error| vec![error])?
  }
  async fn search_inner(
    &self,
    accounts: &AccountRegistry,
    query: &SearchQuery,
    observer: Option<RetryObserver>,
    only: Option<&str>,
  ) -> Result<Answer, Vec<String>> {
    let state = self.get_state();
    if let Some(id) = only {
      match state.providers.get(id) {
        None => return Err(vec![format!("unknown search provider `{id}`")]),
        Some(provider) if !provider.config.enabled => {
          return Err(vec![format!("search provider `{id}` is disabled")]);
        }
        _ => {}
      }
    }
    let order = only.map(|id| vec![id.to_owned()]).unwrap_or_else(|| state.order.clone());
    let mut failures = Vec::new();
    for id in &order {
      let Some(built) = state.providers.get(id) else { continue };
      if !built.config.enabled {
        continue;
      }
      match search_with(built, accounts, query, observer.clone()).await {
        Ok(results) => {
          return Ok(Answer {
            provider: id.clone(),
            name: built.preset["name"].as_str().unwrap_or(id).to_owned(),
            results,
          });
        }
        Err(reason) => failures.push(format!("{id}: {reason}")),
      }
    }
    if failures.is_empty() {
      failures.push("no web search provider is configured and enabled".to_owned());
    }
    Err(failures)
  }

  /// Runs one test search on a saved provider, whatever its place in the order.
  pub async fn check(
    &self,
    id: &str,
    accounts: &AccountRegistry,
  ) -> Result<Value, ApplicationError> {
    let state = self.get_state();
    let built = state.providers.get(id).ok_or_else(ApplicationError::not_found)?;
    let query = SearchQuery { query: "Wish".to_owned(), limit: 5, ..Default::default() };
    let started = Instant::now();
    let results =
      search_with(built, accounts, &query, None).await.map_err(ApplicationError::invalid)?;
    Ok(json!({
      "results": results.results,
      "warnings": results.warnings,
      "duration_ms": started.elapsed().as_millis() as u64,
    }))
  }

  fn get_state(&self) -> Arc<State> {
    self.state.read().unwrap().clone()
  }
}

/// A configuration's providers, built and waiting for the save to succeed.
pub struct PreparedSearch(State);

fn find_preset(
  id: &str,
  config: &SearchProviderConfig,
) -> Result<&'static Value, ApplicationError> {
  presets::find_search(&config.preset).ok_or_else(|| {
    ApplicationError::invalid(format!("search provider {id}: unknown preset {}", config.preset))
  })
}
fn parse_protocol(preset: &Value) -> Result<SearchProtocol, ApplicationError> {
  preset["protocol"]
    .as_str()
    .unwrap_or_default()
    .parse()
    .map_err(|error: crate::protocol::error::Error| ApplicationError::invalid(error.to_string()))
}

fn build(config: &SearchConfig, proxy: &ProxyConfig) -> Result<State, ApplicationError> {
  let mut providers = BTreeMap::new();
  for (id, provider) in &config.providers {
    let preset = find_preset(id, provider)?;
    let protocol = parse_protocol(preset)?;
    let account = match &provider.auth_provider {
      Some(lender) => Account::Borrowed(
        lender.clone(),
        ReqwestTransport::new(
          crate::protocol::attempt::Limits {
            first_byte: TIMEOUT,
            total: TIMEOUT,
            ..Default::default()
          },
          proxy.policy(provider.proxy_enabled),
        )?,
      ),
      None => {
        let mut credentials = Credentials::default();
        if provider.enabled {
          credentials.api_key = provider.api_key.clone().unwrap_or_default();
          if let Some(name) = &provider.api_key_env {
            credentials.api_key = read_secret(name).map_err(ApplicationError::invalid)?;
          }
        }
        let limits = crate::protocol::attempt::Limits {
          first_byte: TIMEOUT,
          total: TIMEOUT,
          ..Default::default()
        };
        let transport = ReqwestTransport::new(limits, proxy.policy(provider.proxy_enabled))
          .map_err(|error| ApplicationError::invalid(error.to_string()))?;
        Account::Own(Box::new(OwnAccount { transport, credentials }))
      }
    };
    providers.insert(id.clone(), Built { config: provider.clone(), preset, protocol, account });
  }
  Ok(State { order: config.order.clone(), providers })
}

/// Why a provider cannot search now, if it cannot.
fn usable(built: &Built, accounts: &AccountRegistry) -> Result<(), String> {
  if !built.config.enabled {
    return Err("switched off".to_owned());
  }
  match &built.account {
    Account::Borrowed(lender, _) => match accounts.get(lender) {
      None => Err(format!("its account {lender} no longer exists")),
      Some(account) if !account.enabled => Err(format!("its account {lender} is off")),
      Some(account) if account.client.get_api_key().is_empty() => {
        Err(format!("its account {lender} is not signed in"))
      }
      Some(_) => Ok(()),
    },
    Account::Own(own) => {
      if built.preset["key"] == "required" && own.credentials.api_key.is_empty() {
        Err("it has no API key".to_owned())
      } else {
        Ok(())
      }
    }
  }
}

async fn search_with(
  built: &Built,
  accounts: &AccountRegistry,
  query: &SearchQuery,
  observer: Option<RetryObserver>,
) -> Result<SearchResults, String> {
  usable(built, accounts)?;
  let mut headers: Vec<(String, String)> =
    built.config.headers.iter().map(|(name, value)| (name.clone(), value.clone())).collect();
  match &built.account {
    Account::Borrowed(lender, transport) => {
      let account = accounts.get(lender).ok_or("its account is gone")?;
      // A service beside the model API shares its base URL; one on a regional host follows the
      // region of the lending provider's preset.
      let base_url = built.config.base_url.clone().filter(|url| !url.is_empty()).or_else(|| {
        account
          .config
          .preset
          .as_deref()
          .and_then(|preset| built.preset["base_url_by_lender"][preset].as_str())
          .map(str::to_owned)
          .or_else(|| {
            (built.preset["base_url_from_provider"] == true)
              .then(|| account.service_base_url.clone())
              .flatten()
          })
      });
      if let Some(agent) = account.get_user_agent() {
        headers.push(("user-agent".to_owned(), agent));
      }
      let client = ReadClient::new(
        transport.clone(),
        account.client.get_credentials().clone(),
        crate::utils::retry::RetryPolicy::auxiliary(),
        observer,
      );
      client
        .search(built.protocol, query, base_url.as_deref(), &headers)
        .await
        .map_err(|error| error.to_string())
    }
    Account::Own(own) => {
      let OwnAccount { transport, credentials } = own.as_ref();
      let base_url = built.config.base_url.as_deref().filter(|url| !url.is_empty());
      if crate::protocol::attempt::find_header(&headers, "user-agent").is_none() {
        headers.push(("user-agent".to_owned(), WISH_USER_AGENT.to_owned()));
      }
      let client = ReadClient::new(
        transport.clone(),
        credentials.clone(),
        crate::utils::retry::RetryPolicy::auxiliary(),
        observer,
      );
      client
        .search(built.protocol, query, base_url, &headers)
        .await
        .map_err(|error| error.to_string())
    }
  }
}
