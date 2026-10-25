//! Shared account material. Services refer to an account; only accounts own renewable secrets.
use super::{config::Config, error::ApiError, presets, provider::ProviderConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct AccountConfig {
  pub name: String,
  pub preset: Option<String>,
  pub api_key: Option<String>,
  pub api_key_env: Option<String>,
  pub refresh_token: Option<String>,
  pub expires_at: Option<u64>,
  pub credentials: BTreeMap<String, String>,
  pub credentials_env: BTreeMap<String, String>,
  /// Network policy used only for signing in and renewing this account.
  #[serde(default = "super::config::yes")]
  pub proxy_enabled: bool,
}
impl Default for AccountConfig {
  fn default() -> Self {
    Self {
      name: String::new(),
      preset: None,
      api_key: None,
      api_key_env: None,
      refresh_token: None,
      expires_at: None,
      credentials: BTreeMap::new(),
      credentials_env: BTreeMap::new(),
      proxy_enabled: true,
    }
  }
}
impl AccountConfig {
  pub fn apply(&self, provider: &mut ProviderConfig) {
    provider.api_key = self.api_key.clone();
    provider.api_key_env = self.api_key_env.clone();
    provider.refresh_token = self.refresh_token.clone();
    provider.expires_at = self.expires_at;
    provider.credentials = self.credentials.clone();
    provider.credentials_env = self.credentials_env.clone();
  }
  pub fn client_config(&self) -> Result<ProviderConfig, ApiError> {
    let variant = self.preset.as_deref().and_then(presets::find_provider)
      .and_then(|p| p["variants"].as_object()).and_then(|v| v.values().next()).cloned()
      .unwrap_or_else(|| serde_json::json!({"protocol":"compatible_chat","base_url":"https://api.openai.com/v1","path":"/chat/completions","auth":"bearer"}));
    let mut config: ProviderConfig = serde_json::from_value(variant).map_err(ApiError::internal)?;
    config.preset = self.preset.clone().filter(|p| presets::find_provider(p).is_some());
    config.proxy_enabled = self.proxy_enabled;
    self.apply(&mut config);
    Ok(config)
  }
}
impl Config {
  /// Canonicalize legacy inline secrets without changing service IDs. Explicit shared references
  /// always win; old clients may update their account through the legacy service secret fields.
  pub fn migrate_accounts(&mut self) -> Result<(), ApiError> {
    for (id, provider) in &mut self.providers {
      let has_material = provider.api_key.is_some()
        || provider.api_key_env.is_some()
        || provider.refresh_token.is_some()
        || !provider.credentials.is_empty()
        || !provider.credentials_env.is_empty();
      if provider.credential_id.is_none() && (has_material || provider.get_renewal().is_some()) {
        let mut account_id = format!("model-{id}");
        while self.accounts.contains_key(&account_id) {
          account_id.push('_');
        }
        provider.credential_id = Some(account_id.clone());
        self.accounts.insert(
          account_id,
          AccountConfig {
            name: provider.display_name.clone().unwrap_or_else(|| id.clone()),
            preset: provider.preset.clone(),
            proxy_enabled: provider.proxy_enabled,
            ..Default::default()
          },
        );
      }
      if let Some(account_id) = &provider.credential_id {
        let account = self
          .accounts
          .get_mut(account_id)
          .ok_or_else(|| ApiError::bad_request(format!("unknown account {account_id}")))?;
        if has_material {
          if provider.api_key.is_some() || provider.api_key_env.is_some() {
            account.api_key = provider.api_key.take();
            account.api_key_env = provider.api_key_env.take();
          }
          if provider.refresh_token.is_some() {
            account.refresh_token = provider.refresh_token.take();
          }
          if provider.expires_at.is_some() {
            account.expires_at = provider.expires_at.take();
          }
          account.credentials.append(&mut provider.credentials);
          account.credentials_env.append(&mut provider.credentials_env);
        }
      }
    }
    for (id, service) in &mut self.search.providers {
      if let Some(lender) = service.auth_provider.take() {
        let spec = presets::find_search(&service.preset)
          .ok_or_else(|| ApiError::bad_request("unknown preset"))?;
        let lenders = spec["borrows_from"]
          .as_array()
          .filter(|v| !v.is_empty())
          .ok_or_else(|| ApiError::bad_request("has no account to borrow"))?;
        if service.api_key.is_some() || service.api_key_env.is_some() {
          return Err(ApiError::bad_request("takes no key of its own"));
        }
        let provider = self
          .providers
          .get(&lender)
          .ok_or_else(|| ApiError::bad_request(format!("borrows missing provider {lender}")))?;
        if !lenders.iter().any(|v| v.as_str() == provider.preset.as_deref()) {
          return Err(ApiError::bad_request(format!("cannot borrow {lender}")));
        }
        service.credential_id = provider.credential_id.clone();
        service.proxy_enabled = provider.proxy_enabled;
        if service.base_url.is_none() {
          if let Some(preset) = presets::find_search(&service.preset) {
            service.base_url = provider
              .preset
              .as_deref()
              .and_then(|p| preset["base_url_by_lender"][p].as_str())
              .map(str::to_owned)
              .or_else(|| {
                (preset["base_url_from_provider"] == true).then(|| provider.base_url.clone())
              });
          }
        }
      }
      if service.credential_id.is_none()
        && (service.api_key.is_some() || service.api_key_env.is_some())
      {
        let mut account_id = format!("search-{id}");
        while self.accounts.contains_key(&account_id) {
          account_id.push('_');
        }
        self.accounts.insert(
          account_id.clone(),
          AccountConfig {
            name: id.clone(),
            api_key: service.api_key.take(),
            api_key_env: service.api_key_env.take(),
            proxy_enabled: service.proxy_enabled,
            ..Default::default()
          },
        );
        service.credential_id = Some(account_id);
      }
      if let Some(id) = &service.credential_id {
        let account = self
          .accounts
          .get_mut(id)
          .ok_or_else(|| ApiError::bad_request(format!("unknown account {id}")))?;
        if service.api_key.is_some() || service.api_key_env.is_some() {
          account.api_key = service.api_key.take();
          account.api_key_env = service.api_key_env.take();
        }
      }
    }
    Ok(())
  }
  pub fn resolve_provider(&self, provider: &ProviderConfig) -> Result<ProviderConfig, ApiError> {
    let mut result = provider.clone();
    if let Some(id) = &provider.credential_id {
      let account = self
        .accounts
        .get(id)
        .ok_or_else(|| ApiError::bad_request(format!("unknown account {id}")))?;
      // Subscription protocol identity must match its account's token exchange.
      if provider.get_renewal().is_some() && provider.preset != account.preset {
        return Err(ApiError::bad_request(
          "subscription account is incompatible with model service",
        ));
      }
      account.apply(&mut result);
    }
    Ok(result)
  }
  pub fn resolved_providers(&self) -> Result<BTreeMap<String, ProviderConfig>, ApiError> {
    self.providers.iter().map(|(id, p)| Ok((id.clone(), self.resolve_provider(p)?))).collect()
  }
  pub fn account_providers(&self) -> Result<BTreeMap<String, ProviderConfig>, ApiError> {
    self
      .accounts
      .iter()
      .map(|(id, account)| {
        let mut config = account.client_config()?;
        let in_use =
          self.providers.values().any(|p| p.enabled && p.credential_id.as_ref() == Some(id))
            || self
              .search
              .providers
              .values()
              .any(|p| p.enabled && p.credential_id.as_ref() == Some(id))
            || self
              .fetch
              .providers
              .values()
              .any(|p| p.enabled && p.credential_id.as_ref() == Some(id));
        // An unused account can name environment secrets not installed on this host yet,
        // just as disabled legacy services could. Activating a service validates them.
        let environment_ready = account
          .api_key_env
          .iter()
          .chain(account.credentials_env.values())
          .all(|name| std::env::var(name).is_ok());
        config.enabled = in_use || environment_ready;
        Ok((format!("account:{id}"), config))
      })
      .collect()
  }
  pub fn resolved_search(&self) -> Result<super::search::SearchConfig, ApiError> {
    let mut search = self.search.clone();
    for provider in search.providers.values_mut() {
      if let Some(id) = &provider.credential_id {
        let account = self
          .accounts
          .get(id)
          .ok_or_else(|| ApiError::bad_request(format!("unknown account {id}")))?;
        let preset = presets::find_search(&provider.preset)
          .ok_or_else(|| ApiError::bad_request("unknown search preset"))?;
        if preset["borrows_from"].as_array().is_some_and(|a| !a.is_empty()) {
          provider.auth_provider = Some(format!("account:{id}"));
        } else {
          provider.api_key = account.api_key.clone();
          provider.api_key_env = account.api_key_env.clone();
        }
      }
    }
    Ok(search)
  }
}
