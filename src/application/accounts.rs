//! Shared account material. Services refer to an account; only accounts own renewable secrets.
use super::{config::Config, error::ApplicationError, presets, provider::ProviderConfig};
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
  pub fn is_codex(&self) -> bool {
    self.preset.as_deref() == Some("openai_codex")
  }
  pub fn is_copilot(&self) -> bool {
    self.preset.as_deref() == Some("github_copilot")
  }
  pub fn get_renewal(&self) -> Option<crate::protocol::endpoint::CredentialRenewal> {
    use crate::protocol::endpoint::CredentialRenewal;
    match self.preset.as_deref() {
      Some("openai_codex") => Some(CredentialRenewal::CodexOAuth),
      Some("github_copilot") => Some(CredentialRenewal::CopilotToken),
      _ => None,
    }
  }
}
impl Config {
  /// Canonicalize legacy inline secrets without changing service IDs. Explicit shared references
  /// always win; old clients may update their account through the legacy service secret fields.
  pub fn migrate_accounts(&mut self) -> Result<(), ApplicationError> {
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
          .ok_or_else(|| ApplicationError::invalid(format!("unknown account {account_id}")))?;
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
          .ok_or_else(|| ApplicationError::invalid("unknown preset"))?;
        let lenders = spec["borrows_from"]
          .as_array()
          .filter(|v| !v.is_empty())
          .ok_or_else(|| ApplicationError::invalid("has no account to borrow"))?;
        if service.api_key.is_some() || service.api_key_env.is_some() {
          return Err(ApplicationError::invalid("takes no key of its own"));
        }
        let provider = self
          .providers
          .get(&lender)
          .ok_or_else(|| ApplicationError::invalid(format!("borrows missing provider {lender}")))?;
        if !lenders.iter().any(|v| v.as_str() == provider.preset.as_deref()) {
          return Err(ApplicationError::invalid(format!("cannot borrow {lender}")));
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
          .ok_or_else(|| ApplicationError::invalid(format!("unknown account {id}")))?;
        if service.api_key.is_some() || service.api_key_env.is_some() {
          account.api_key = service.api_key.take();
          account.api_key_env = service.api_key_env.take();
        }
      }
    }
    Ok(())
  }
  pub fn resolve_provider(
    &self,
    provider: &ProviderConfig,
  ) -> Result<ProviderConfig, ApplicationError> {
    let mut result = provider.clone();
    if let Some(id) = &provider.credential_id {
      let account = self
        .accounts
        .get(id)
        .ok_or_else(|| ApplicationError::invalid(format!("unknown account {id}")))?;
      // Subscription protocol identity must match its account's token exchange.
      if provider.get_renewal().is_some() && provider.preset != account.preset {
        return Err(ApplicationError::invalid(
          "subscription account is incompatible with model service",
        ));
      }
      account.apply(&mut result);
    }
    Ok(result)
  }
  pub fn resolved_providers(&self) -> Result<BTreeMap<String, ProviderConfig>, ApplicationError> {
    self.providers.iter().map(|(id, p)| Ok((id.clone(), self.resolve_provider(p)?))).collect()
  }
  pub fn build_accounts(&self) -> Result<AccountRegistry, ApplicationError> {
    self
      .accounts
      .iter()
      .map(|(id, config)| {
        let in_use = self
          .providers
          .values()
          .any(|service| service.enabled && service.credential_id.as_ref() == Some(id))
          || self
            .search
            .providers
            .values()
            .any(|service| service.enabled && service.credential_id.as_ref() == Some(id))
          || self
            .fetch
            .providers
            .values()
            .any(|service| service.enabled && service.credential_id.as_ref() == Some(id));
        let environment_ready = config
          .api_key_env
          .iter()
          .chain(config.credentials_env.values())
          .all(|name| std::env::var(name).is_ok());
        let key = format!("account:{id}");
        Ok((
          key.clone(),
          std::sync::Arc::new(AccountRuntime::build(
            key,
            config.clone(),
            in_use || environment_ready,
            &self.proxy,
          )?),
        ))
      })
      .collect()
  }
  pub fn resolved_search(&self) -> Result<super::search::SearchConfig, ApplicationError> {
    let mut search = self.search.clone();
    for provider in search.providers.values_mut() {
      if let Some(id) = &provider.credential_id {
        let account = self
          .accounts
          .get(id)
          .ok_or_else(|| ApplicationError::invalid(format!("unknown account {id}")))?;
        let preset = presets::find_search(&provider.preset)
          .ok_or_else(|| ApplicationError::invalid("unknown search preset"))?;
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

/// Credentials and renewal state shared by services; no model endpoint or model protocol.
pub struct AccountRuntime {
  pub id: String,
  pub config: AccountConfig,
  pub enabled: bool,
  pub client: crate::client::account::AccountClient<crate::transport::ReqwestTransport>,
  pub service_base_url: Option<String>,
}
pub type AccountRegistry = BTreeMap<String, std::sync::Arc<AccountRuntime>>;
impl AccountRuntime {
  pub fn build(
    id: String,
    config: AccountConfig,
    enabled: bool,
    proxy: &super::config::ProxyConfig,
  ) -> Result<Self, ApplicationError> {
    use crate::protocol::endpoint::{CredentialField, Credentials};
    let mut credentials = Credentials::default();
    if enabled {
      credentials.api_key = match &config.api_key_env {
        Some(name) => super::provider::read_secret(name).map_err(ApplicationError::invalid)?,
        None => config.api_key.clone().unwrap_or_default(),
      };
      credentials.refresh_token = config.refresh_token.clone();
      credentials.expires_at = config.expires_at;
    }
    let mut fields = config.credentials.clone();
    if enabled {
      for (field, name) in &config.credentials_env {
        fields.insert(
          field.clone(),
          super::provider::read_secret(name).map_err(ApplicationError::invalid)?,
        );
      }
    }
    for (field, value) in fields {
      match CredentialField::from_name(&field) {
        Some(field) if field != CredentialField::ApiKey => credentials.set_field(field, value),
        _ => return Err(ApplicationError::invalid(format!("unknown credential: {field}"))),
      }
    }
    let transport = crate::transport::ReqwestTransport::new(
      Default::default(),
      proxy.policy(config.proxy_enabled),
    )?;
    let client =
      crate::client::account::AccountClient::new(transport, credentials, config.get_renewal());
    let service_base_url = config
      .preset
      .as_deref()
      .and_then(presets::find_provider)
      .and_then(|preset| preset["variants"].as_object())
      .and_then(|variants| variants.values().next())
      .and_then(|variant| variant["base_url"].as_str())
      .map(str::to_owned);
    Ok(Self { id, config, enabled, client, service_base_url })
  }
  pub fn get_user_agent(&self) -> Option<String> {
    self.config.is_codex().then(|| super::provider::CODEX_USER_AGENT.to_owned())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn reader_account_does_not_require_a_model_service() {
    let runtime = AccountRuntime::build(
      "account:reader".into(),
      AccountConfig {
        preset: Some("jina_reader".into()),
        api_key: Some("reader-key".into()),
        ..Default::default()
      },
      true,
      &Default::default(),
    )
    .unwrap();
    assert_eq!(runtime.client.get_api_key(), "reader-key");
    assert!(runtime.config.get_renewal().is_none());
    assert!(runtime.service_base_url.is_none());
  }
}
