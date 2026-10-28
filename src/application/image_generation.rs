//! Configured image services: one paid submission, bounded polling, then session-local blobs.
use super::{
  accounts::{AccountConfig, AccountRegistry},
  config::ProxyConfig,
  error::ApplicationError,
};
use crate::{
  protocol::{
    attempt::{Call, Limits, Method, Transport},
    image_generation::{self as wire, ImageProtocol, ImageReply, ImageSource},
  },
  storage::blobs,
  transport::ReqwestTransport,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
  collections::BTreeMap,
  path::Path,
  sync::{Arc, RwLock},
  time::Duration,
};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImageConfig {
  pub order: Vec<String>,
  pub providers: BTreeMap<String, ImageProvider>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageProvider {
  pub preset: String,
  pub credential_id: String,
  pub model: String,
  #[serde(default)]
  pub base_url: Option<String>,
  #[serde(default = "super::config::yes")]
  pub enabled: bool,
  #[serde(default = "super::config::yes")]
  pub proxy_enabled: bool,
  #[serde(default)]
  pub parameters: serde_json::Map<String, Value>,
}
#[derive(Clone, Deserialize, Serialize)]
pub struct ImagePreset {
  pub id: String,
  pub name: String,
  pub brand: String,
  pub protocol: ImageProtocol,
  pub base_url: String,
  pub path: String,
  pub model: String,
  pub account_presets: Vec<String>,
  #[serde(default)]
  pub parameters: serde_json::Map<String, Value>,
  pub docs_url: String,
}
pub fn presets() -> &'static Vec<ImagePreset> {
  static PRESETS: std::sync::OnceLock<Vec<ImagePreset>> = std::sync::OnceLock::new();
  PRESETS.get_or_init(|| {
    serde_json::from_str(include_str!("../../resources/image-presets.json"))
      .expect("valid image presets")
  })
}
fn preset(id: &str) -> Result<&'static ImagePreset, ApplicationError> {
  presets()
    .iter()
    .find(|p| p.id == id)
    .ok_or_else(|| ApplicationError::invalid(format!("unknown image preset {id}")))
}
impl ImageConfig {
  pub fn validate(
    &self,
    accounts: &BTreeMap<String, AccountConfig>,
  ) -> Result<(), ApplicationError> {
    let mut seen = std::collections::HashSet::new();
    for id in &self.order {
      if !self.providers.contains_key(id) || !seen.insert(id) {
        return Err(ApplicationError::invalid("invalid image provider order"));
      }
    }
    for p in self.providers.values() {
      let spec = preset(&p.preset)?;
      let account = accounts
        .get(&p.credential_id)
        .ok_or_else(|| ApplicationError::invalid("image service requires an account"))?;
      if spec.protocol == ImageProtocol::Codex && !account.is_codex() {
        return Err(ApplicationError::invalid(
          "subscription image generation requires a ChatGPT account",
        ));
      }
      if spec.protocol != ImageProtocol::Codex && account.is_codex() {
        return Err(ApplicationError::invalid(
          "ChatGPT subscription credentials cannot be used as an image API key",
        ));
      }
      if p.model.is_empty()
        || p.model.len() > 200
        || !p.model.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/".contains(c))
        || p.model.contains("..")
      {
        return Err(ApplicationError::invalid("invalid image model ID"));
      }
      wire::validate_url(
        &p.base_url
          .as_deref()
          .filter(|s| !s.trim().is_empty())
          .unwrap_or(&spec.base_url)
          .replace("{workspace_id}", "workspace"),
      )?;
      if serde_json::to_vec(&p.parameters).map_err(ApplicationError::internal)?.len() > 16 * 1024 {
        return Err(ApplicationError::invalid("image parameters exceed 16 KiB"));
      }
      for key in ["n", "num_images", "batch_size"] {
        if p.parameters.get(key).is_some_and(|v| !v.as_u64().is_some_and(|n| (1..=8).contains(&n)))
        {
          return Err(ApplicationError::invalid("image count must be between 1 and 8"));
        }
      }
      for key in ["prompt", "model", "input", "stream", "webhook", "webhook_url"] {
        if p.parameters.contains_key(key) {
          return Err(ApplicationError::invalid(format!(
            "image parameter {key} is managed by Wish"
          )));
        }
      }
    }
    Ok(())
  }
}
struct Built {
  config: ImageProvider,
  spec: ImagePreset,
  transport: ReqwestTransport,
}
struct State {
  order: Vec<String>,
  providers: BTreeMap<String, Built>,
}
pub struct PreparedImages(State);
pub struct ImageHub {
  state: RwLock<Arc<State>>,
  permits: tokio::sync::Semaphore,
}
impl ImageHub {
  pub fn new(config: &ImageConfig, proxy: &ProxyConfig) -> Result<Self, ApplicationError> {
    Ok(Self {
      state: RwLock::new(Arc::new(Self::prepare(config, proxy)?.0)),
      permits: tokio::sync::Semaphore::new(2),
    })
  }
  pub fn prepare(
    config: &ImageConfig,
    proxy: &ProxyConfig,
  ) -> Result<PreparedImages, ApplicationError> {
    let mut providers = BTreeMap::new();
    for (id, p) in &config.providers {
      let transport = ReqwestTransport::new(
        Limits {
          first_byte: Duration::from_secs(300),
          total: Duration::from_secs(300),
          max_response_bytes: 48 * 1024 * 1024,
          ..Default::default()
        },
        proxy.policy(p.proxy_enabled),
      )?;
      providers.insert(
        id.clone(),
        Built { config: p.clone(), spec: preset(&p.preset)?.clone(), transport },
      );
    }
    Ok(PreparedImages(State { order: config.order.clone(), providers }))
  }
  pub fn apply(&self, prepared: PreparedImages) {
    *self.state.write().unwrap() = Arc::new(prepared.0);
  }
  pub async fn generate(
    &self,
    prompt: &str,
    only: Option<&str>,
    accounts: &AccountRegistry,
    directory: &Path,
  ) -> Result<Value, String> {
    if prompt.trim().is_empty() || prompt.len() > 32 * 1024 {
      return Err("prompt must contain 1–32768 bytes".into());
    }
    let _permit = self.permits.acquire().await.map_err(|_| "image service closed")?;
    let state = self.state.read().unwrap().clone();
    let id = only
      .or_else(|| {
        state
          .order
          .iter()
          .find(|id| state.providers.get(*id).is_some_and(|p| p.config.enabled))
          .map(String::as_str)
      })
      .ok_or("no image generation service is configured")?;
    let service = state
      .providers
      .get(id)
      .filter(|p| p.config.enabled)
      .ok_or("image generation service is unavailable")?;
    let account = accounts
      .get(&format!("account:{}", service.config.credential_id))
      .ok_or("image account is unavailable")?;
    let credentials = account.client.get_credentials();
    if credentials.api_key.is_empty() {
      return Err("image account has no API key or active login".into());
    }
    // Never retry/fail over a paid POST: a disconnected response does not prove generation failed.
    let job = async {
      let spec = &service.spec;
      let mut parameters = spec.parameters.clone();
      parameters.extend(service.config.parameters.clone());
      let draft = spec
        .protocol
        .submission(&service.config.model, prompt, &parameters)
        .map_err(|e| e.to_string())?;
      let path = spec.path.replace("{model}", &service.config.model);
      let mut initial = spec
        .protocol
        .call(
          service
            .config
            .base_url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&spec.base_url),
          &path,
          credentials,
          draft,
        )
        .map_err(|e| e.to_string())?;
      initial.headers.push((
        "user-agent".into(),
        account.get_user_agent().unwrap_or_else(|| super::identity::WISH_USER_AGENT.into()),
      ));
      let mut call = initial.clone();
      let mut result_url = None;
      let images = loop {
        let reply=service.transport.execute(&call).await.map_err(|_|"Image request interrupted; upstream outcome may be unknown. Do not automatically submit it again.".to_string())?;
        let body = crate::protocol::http_error::read_provider_json(reply, "image_generation")
          .map_err(|e| e.to_string())?;
        match spec
          .protocol
          .decode(&body, &call.url, result_url.as_deref())
          .map_err(|e| e.to_string())?
        {
          ImageReply::Complete(images) => break images,
          ImageReply::Pending { url, result_url: next } => {
            call = wire::poll_call(spec.protocol, &initial, &url).map_err(|e| e.to_string())?;
            result_url = next;
            tokio::time::sleep(Duration::from_secs(2)).await;
          }
        }
      };
      let mut stored = Vec::new();
      let mut total = 0;
      for image in images {
        let bytes = match image {
          ImageSource::Base64(data) => crate::utils::blocking::cpu(move || {
            base64::engine::general_purpose::STANDARD.decode(&data)
          })
          .await
          .map_err(|e| e.to_string())?
          .map_err(|_| "invalid generated image encoding".to_string())?,
          ImageSource::Url(url) => download(&service.transport, &url).await?,
        };
        total += bytes.len();
        if total > 64 * 1024 * 1024 || bytes.len() > 32 * 1024 * 1024 {
          return Err("generated images exceed storage limit".into());
        }
        if crate::utils::image_format::supported_mime(&bytes).is_none() {
          return Err("provider returned an unsupported image format".into());
        }
        let blob = blobs::save_upload(directory, &bytes).await.map_err(|e| e.to_string())?;
        stored.push(blob);
      }
      Ok(json!({"provider":id,"model":service.config.model,"images":stored}))
    };
    tokio::time::timeout(Duration::from_secs(600),job).await.map_err(|_|"Image generation timed out; upstream may still be running. Do not automatically submit it again.".to_string())?
  }
}
async fn download(transport: &ReqwestTransport, url: &str) -> Result<Vec<u8>, String> {
  let mut url = wire::validate_url(url).map_err(|e| e.to_string())?;
  // CDN requests and redirects never receive account credentials.
  for _ in 0..5 {
    let reply = transport
      .execute(&Call {
        method: Method::Get,
        url: url.to_string(),
        headers: vec![("user-agent".into(), super::identity::WISH_USER_AGENT.into())],
        body: Vec::new(),
      })
      .await
      .map_err(|e| e.to_string())?;
    if reply.is_success() {
      return Ok(reply.body);
    }
    if matches!(reply.status, 301 | 302 | 303 | 307 | 308) {
      let next = url
        .join(reply.get_header("location").ok_or("image redirect has no location")?)
        .map_err(|_| "invalid image redirect")?;
      url = wire::validate_url(next.as_str()).map_err(|e| e.to_string())?;
    } else {
      return Err(format!("generated image download failed (HTTP {})", reply.status));
    }
  }
  Err("too many generated image redirects".into())
}
