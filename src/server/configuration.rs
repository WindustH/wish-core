use crate::server::{app::App, config::Config, error::ApiError, provider::Provider};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub struct Configuration {
  pub path: PathBuf,
  pub config: Config,
  pub revision: String,
}
fn hash(bytes: &[u8]) -> String {
  format!("{:x}", Sha256::digest(bytes))
}
impl Configuration {
  pub fn new(path: PathBuf, config: Config) -> Result<Self, ApiError> {
    let bytes = std::fs::read(&path).map_err(ApiError::internal)?;
    Ok(Self { path, config, revision: hash(&bytes) })
  }
  pub fn describe(&self) -> Value {
    let mut config = serde_json::to_value(&self.config).unwrap();
    for (_, provider) in config["providers"].as_object_mut().unwrap() {
      if !provider["api_key"].is_null() {
        provider["api_key"] = json!("<redacted>");
      }
      if !provider["refresh_token"].is_null() {
        provider["refresh_token"] = json!("<redacted>");
      }
      for (_, secret) in provider["credentials"].as_object_mut().unwrap() {
        *secret = json!("<redacted>");
      }
      for (_, header) in provider["headers"].as_object_mut().unwrap() {
        *header = json!("<redacted>");
      }
    }
    if !self.config.proxy.password.is_empty() {
      config["proxy"]["password"] = json!("<redacted>");
    }
    json!({"revision":self.revision,"config":config})
  }
}
impl App {
  /// Replace one Codex provider's OAuth material without exposing it through the config API.
  pub async fn persist_codex_credentials(
    &self,
    id: &str,
    credentials: &crate::protocol::outbound::Credentials,
  ) -> Result<(), ApiError> {
    for _ in 0..3 {
      let (revision, mut next) = {
        let current = self.configuration.lock().await;
        (current.revision.clone(), current.config.clone())
      };
      let provider = next.providers.get_mut(id).ok_or_else(ApiError::not_found)?;
      if provider.preset.as_deref() != Some("openai_codex") {
        return Err(ApiError::bad_request("provider is not an OpenAI Codex preset"));
      }
      provider.api_key = Some(credentials.api_key.clone());
      provider.api_key_env = None;
      if let Some(refresh_token) = &credentials.refresh_token {
        provider.refresh_token = Some(refresh_token.clone());
      }
      provider.expires_at = credentials.expires_at;
      if let Some(account_id) = &credentials.account_id {
        provider.credentials.insert("account_id".to_owned(), account_id.clone());
        provider.credentials_env.remove("account_id");
      }
      let value = serde_json::to_value(next).map_err(ApiError::internal)?;
      match self.save_configuration(revision, value).await {
        Ok(_) => return Ok(()),
        Err(error) if error.status == axum::http::StatusCode::CONFLICT => continue,
        Err(error) => return Err(error),
      }
    }
    Err(ApiError::conflict("configuration kept changing during Codex login"))
  }

  pub async fn save_configuration(
    &self,
    revision: String,
    mut value: Value,
  ) -> Result<Value, ApiError> {
    let mut current = self.configuration.lock().await;
    let source = tokio::fs::read(&current.path).await.map_err(ApiError::internal)?;
    if revision != current.revision || hash(&source) != revision {
      return Err(ApiError::conflict("configuration changed; reload before saving"));
    }
    if let Some(providers) = value["providers"].as_object_mut() {
      for (id, provider) in providers {
        if provider["api_key"] == "<redacted>" {
          provider["api_key"] = json!(
            current
              .config
              .providers
              .get(id)
              .and_then(|p| p.api_key.as_ref())
              .ok_or_else(|| ApiError::bad_request("redacted API key has no stored value"))?
          );
        }
        if provider["refresh_token"] == "<redacted>" {
          provider["refresh_token"] =
            json!(
              current.config.providers.get(id).and_then(|p| p.refresh_token.as_ref()).ok_or_else(
                || ApiError::bad_request("redacted refresh token has no stored value")
              )?
            );
        }
        if let Some(values) = provider["credentials"].as_object_mut() {
          for (name, value) in values {
            if value == "<redacted>" {
              *value = json!(
                current.config.providers.get(id).and_then(|p| p.credentials.get(name)).ok_or_else(
                  || ApiError::bad_request("redacted credential has no stored value")
                )?
              );
            }
          }
        }
        if let Some(headers) = provider["headers"].as_object_mut() {
          for (name, header) in headers {
            if header == "<redacted>" {
              *header = json!(
                current
                  .config
                  .providers
                  .get(id)
                  .and_then(|p| p.headers.get(name))
                  .ok_or_else(|| ApiError::bad_request("redacted header has no stored value"))?
              );
            }
          }
        }
      }
    }
    if value["proxy"]["password"] == "<redacted>" {
      value["proxy"]["password"] = json!(current.config.proxy.password);
    }
    let next: Config =
      serde_json::from_value(value).map_err(|e| ApiError::bad_request(e.to_string()))?;
    next.proxy.validate()?;
    let shell = next.shell.resolve()?;
    if next.listen != current.config.listen
      || next.data_dir != current.config.data_dir
      || next.bearer_token_env != current.config.bearer_token_env
    {
      return Err(ApiError::bad_request(
        "listen, data_dir and bearer_token_env are startup settings; edit the file and restart",
      ));
    }
    let mut providers = BTreeMap::new();
    for (id, config) in &next.providers {
      let mut provider = Provider::build(config.clone(), &next.proxy)?;
      provider.client = crate::server::sampling::observe_client(
        &provider.client,
        self.index.clone(),
        self.tasks.clone(),
        id.clone(),
        None,
      );
      providers.insert(id.clone(), Arc::new(provider));
    }
    if !next.defaults.provider.is_empty() && !providers.contains_key(&next.defaults.provider) {
      return Err(ApiError::bad_request("default provider does not exist"));
    }
    if !next.defaults.cwd.is_absolute() {
      return Err(ApiError::bad_request("default cwd must be absolute"));
    }
    // Validate core budgets without creating a persistent session.
    let _ = crate::session::Session::new(next.defaults.session_config())?;
    let bytes = serde_json::to_vec_pretty(&next).map_err(ApiError::internal)?;
    let temp = current.path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let permissions =
      tokio::fs::metadata(&current.path).await.map_err(ApiError::internal)?.permissions();
    tokio::fs::write(&temp, &bytes).await.map_err(ApiError::internal)?;
    tokio::fs::set_permissions(&temp, permissions).await.map_err(ApiError::internal)?;
    if let Err(error) = tokio::fs::rename(&temp, &current.path).await {
      let _ = tokio::fs::remove_file(&temp).await;
      return Err(ApiError::internal(error));
    }
    *self.providers.write().unwrap() = providers;
    *self.shell.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = shell.clone();
    for slot in self.sessions.lock().await.values() {
      slot.follow_global_shell(&shell);
    }
    current.config = next;
    current.revision = hash(&bytes);
    let _ = self.events.send(json!({"type":"configuration_changed"}));
    Ok(current.describe())
  }
}
