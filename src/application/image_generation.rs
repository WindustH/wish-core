//! Configured image services: one paid submission, bounded polling, then session-local blobs.
use super::{
  accounts::{AccountConfig, AccountRegistry},
  config::ProxyConfig,
  error::ApplicationError,
};
use crate::extension_host::images::{self as wire, ImageProtocol, ImageSource};
use crate::{protocol::attempt::Limits, storage::blobs, transport::ReqwestTransport};
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
  #[serde(default)]
  pub required_credentials: Vec<String>,
  #[serde(default)]
  pub optional_credentials: Vec<String>,
  #[serde(default)]
  pub reference_images: bool,
  #[serde(default)]
  pub reference_image_limit: usize,
  #[serde(default)]
  pub mask_editing: bool,
  #[serde(default)]
  pub markup_editing: bool,
  #[serde(default)]
  pub image_required: bool,
}
pub fn presets() -> &'static Vec<ImagePreset> {
  static PRESETS: std::sync::OnceLock<Vec<ImagePreset>> = std::sync::OnceLock::new();
  PRESETS.get_or_init(|| {
    let mut presets: Vec<ImagePreset> =
      serde_json::from_value(serde_json::json!(crate::extensions::package::presets("image")))
        .expect("valid image presets");
    for preset in &mut presets {
      preset.reference_images = preset.protocol.reference_images(&preset.model);
      preset.reference_image_limit = preset.protocol.reference_image_limit(&preset.model);
      preset.markup_editing = preset.protocol.markup_editing(&preset.model);
      preset.mask_editing = preset.protocol.mask_editing(&preset.model);
      preset.image_required = preset.protocol.image_required(&preset.model);
    }
    presets
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
      if spec.protocol.subscription() && !account.is_codex() {
        return Err(ApplicationError::invalid(
          "subscription image generation requires a ChatGPT account",
        ));
      }
      if !spec.protocol.subscription() && account.is_codex() {
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
      crate::extensions::shared::urls::validate_base_url(
        &p.base_url
          .as_deref()
          .filter(|s| !s.trim().is_empty())
          .unwrap_or(&spec.base_url)
          .replace("{workspace_id}", "workspace"),
      )?;
      if serde_json::to_vec(&p.parameters).map_err(ApplicationError::internal)?.len() > 16 * 1024 {
        return Err(ApplicationError::invalid("image parameters exceed 16 KiB"));
      }
      let max_count = spec.protocol.max_output_images();
      for key in ["n", "num_images", "batch_size", "n_samples"] {
        if p
          .parameters
          .get(key)
          .is_some_and(|v| !v.as_u64().is_some_and(|n| (1..=max_count).contains(&n)))
        {
          return Err(ApplicationError::invalid(format!(
            "image count must be between 1 and {max_count}"
          )));
        }
      }
      let mut edit_parameters = spec.parameters.clone();
      edit_parameters.extend(p.parameters.clone());
      spec.protocol.0.validate_configuration(&p.model, &edit_parameters)?;
      for key in [
        "prompt",
        "model",
        "input",
        "stream",
        "webhook",
        "webhook_url",
        "image",
        "images",
        "image_urls",
        "mask",
        "markup",
      ] {
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
impl Built {
  fn accepts_options(&self, options: &serde_json::Map<String, Value>) -> bool {
    if !self.spec.protocol.accepts_options(&self.config.model, options) {
      return false;
    }
    let mut parameters = self.spec.parameters.clone();
    parameters.extend(self.config.parameters.clone());
    self.spec.protocol.merge_options(&self.config.model, parameters, options).is_ok()
  }
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
  pub fn provider_catalog(&self, accounts: &AccountRegistry) -> crate::tool::providers::Catalog {
    let state = self.state.read().unwrap();
    crate::tool::providers::Catalog {
      default_order: state.order.clone(),
      failure_policy: "first_enabled_compatible_only_no_paid_submission_failover",
      providers: state
        .providers
        .iter()
        .map(|(id, built)| crate::tool::providers::Provider {
          id: id.clone(),
          name: built.spec.name.clone(),
          preset: built.config.preset.clone(),
          kind: None,
          model: Some(built.config.model.clone()),
          markup_editing: Some(built.spec.protocol.markup_editing(&built.config.model)),
          mask_editing: Some(built.spec.protocol.mask_editing(&built.config.model)),
          image_required: Some(built.spec.protocol.image_required(&built.config.model)),
          reference_images: Some(built.spec.protocol.reference_images(&built.config.model)),
          image_options: Some(built.spec.protocol.request_options(&built.config.model)),
          precise_references: Some(built.spec.protocol.precise_references(&built.config.model)),
          video: None,
          enabled: built.config.enabled,
          available: built.config.enabled
            && accounts
              .get(&format!("account:{}", built.config.credential_id))
              .is_some_and(|a| built.spec.protocol.credentials_ready(a.client.get_credentials())),
        })
        .collect(),
    }
  }
  pub async fn generate(
    &self,
    request: &crate::tool::image_generate::ImageRequest,
    accounts: &AccountRegistry,
    directory: &Path,
  ) -> Result<Value, crate::tool::paid_operation::Failure> {
    let crate::tool::image_generate::ImageRequest {
      prompt,
      images: image_ids,
      mask,
      markup,
      options,
      provider,
      references,
    } = request;
    let only = provider.as_deref();
    if mask.is_some() && markup.is_some() {
      return Err("mask and markup cannot be combined".into());
    }
    let overlay_id = mask.as_ref().or(markup.as_ref());
    if overlay_id.is_some_and(|id| {
      !blobs::is_blob_id(id) || image_ids.is_empty() || !blobs::is_blob_id(&image_ids[0])
    }) {
      return Err("mask/markup requires session blob IDs and a local first source image".into());
    }
    if references.len() > 8
      || references.iter().any(|r| {
        !blobs::is_blob_id(&r.image)
          || !(0.0..=1.0).contains(&r.strength)
          || !(0.0..=1.0).contains(&r.fidelity)
      })
    {
      return Err(
        "precise references require at most eight session blob IDs, with strength/fidelity in 0..1"
          .into(),
      );
    }
    if prompt.trim().is_empty() || prompt.len() > 32 * 1024 {
      return Err("prompt must contain 1–32768 bytes".into());
    }
    if image_ids.len() > 16 {
      return Err("at most sixteen reference images are supported".into());
    }
    // Bound input size here; each extension owns its supported parameter set.
    if serde_json::to_vec(options).map_err(|e| e.to_string())?.len() > 40 * 1024 {
      return Err("image options exceed the size limit".into());
    }
    if image_ids.is_empty() && options.contains_key("input_fidelity") {
      return Err("input_fidelity requires source images".into());
    }
    if image_ids.is_empty() && (options.contains_key("strength") || options.contains_key("noise")) {
      return Err("strength and noise require a source image".into());
    }
    let _permit = self.permits.acquire().await.map_err(|_| "image service closed")?;
    let state = self.state.read().unwrap().clone();
    let id = only
      .or_else(|| {
        state
          .order
          .iter()
          .find(|id| {
            state.providers.get(*id).is_some_and(|p| {
              p.config.enabled
                && accounts
                  .get(&format!("account:{}", p.config.credential_id))
                  .is_some_and(|a| p.spec.protocol.credentials_ready(a.client.get_credentials()))
                && (mask.is_none() || p.spec.protocol.mask_editing(&p.config.model))
                && (markup.is_none() || p.spec.protocol.markup_editing(&p.config.model))
                && (references.is_empty() || p.spec.protocol.precise_references(&p.config.model))
                && p.accepts_options(options)
                && p.spec.protocol.accepts_reference_count(&p.config.model, image_ids.len())
                && image_ids.iter().all(|id| p.spec.protocol.accepts_input(&p.config.model, id))
            })
          })
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
    if !service.spec.protocol.credentials_ready(credentials) {
      return Err("image account lacks required credentials".into());
    }
    // Never retry/fail over a paid POST: a disconnected response does not prove generation failed.
    let submitted = std::sync::atomic::AtomicBool::new(false);
    let job = async {
      let spec = &service.spec;
      let mut parameters = spec.parameters.clone();
      parameters.extend(service.config.parameters.clone());
      if !spec.protocol.accepts_options(&service.config.model, options) {
        return Err("selected image provider does not support these options; check list_providers image_options".into());
      }
      let parameters = spec
        .protocol
        .merge_options(&service.config.model, parameters, options)
        .map_err(|e| e.to_string())?;
      if !spec.protocol.accepts_reference_count(&service.config.model, image_ids.len())
        || image_ids.iter().any(|id| !spec.protocol.accepts_input(&service.config.model, id))
      {
        return Err("selected image provider does not support this image count or source type; NovelAI accepts one blob, Jimeng accepts HTTPS URLs".into());
      }
      if !references.is_empty() && !spec.protocol.precise_references(&service.config.model) {
        return Err("selected provider/model does not support precise references".into());
      }
      if mask.is_some() && !spec.protocol.mask_editing(&service.config.model) {
        return Err("selected image provider/model does not support mask editing".into());
      }
      if markup.is_some() && !spec.protocol.markup_editing(&service.config.model) {
        return Err("selected image provider/model does not support markup editing".into());
      }
      let ids = image_ids
        .iter()
        .cloned()
        .chain(references.iter().map(|r| r.image.clone()))
        .chain(overlay_id.cloned())
        .collect::<Vec<_>>();
      let mut images = load_references(directory, &ids).await?;
      let overlay = if overlay_id.is_some() {
        match images.pop() {
          Some(wire::ReferenceInput::Inline(image)) => Some(if mask.is_some() {
            wire::EditOverlay::Mask(image)
          } else {
            wire::EditOverlay::Markup(image)
          }),
          _ => return Err("mask/markup must be a session image blob".into()),
        }
      } else {
        None
      };
      let driver = spec.protocol;
      let prepare_model = service.config.model.clone();
      let prepare_parameters = parameters.clone();
      let (mut images, overlay) = crate::utils::blocking::cpu(move || {
        let overlay =
          driver.0.prepare(&prepare_model, &prepare_parameters, &mut images, overlay)?;
        Ok::<_, String>((images, overlay))
      })
      .await
      .map_err(|e| e.to_string())??;
      let reference_images = images.split_off(image_ids.len());
      let settings =
        references.iter().map(|r| (r.kind, r.strength, r.fidelity)).collect::<Vec<_>>();
      let prepared = if reference_images.is_empty() {
        Vec::new()
      } else {
        crate::utils::blocking::cpu(move || driver.0.prepare_references(reference_images, settings))
          .await
          .map_err(|e| e.to_string())??
      };
      let images = spec
        .protocol
        .0
        .upload(
          images,
          &service.transport,
          service
            .config
            .base_url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&spec.base_url),
          credentials,
        )
        .await?;
      let draft = spec
        .protocol
        .submission_with_images(
          &service.config.model,
          prompt,
          &parameters,
          &images,
          &prepared,
          overlay.as_ref(),
        )
        .map_err(|e| e.to_string())?;
      let path = spec.path.replace("{model}", &service.config.model);
      let user_agent =
        account.get_user_agent().unwrap_or_else(|| super::identity::WISH_USER_AGENT.into());
      let execution = wire::execution::Execution {
        protocol: spec.protocol,
        model: &service.config.model,
        base_url: service
          .config
          .base_url
          .as_deref()
          .filter(|s| !s.trim().is_empty())
          .unwrap_or(&spec.base_url),
        path: &path,
        credentials,
        parameters: &parameters,
        user_agent: &user_agent,
      };
      let images = execution.run(&service.transport, draft, &submitted).await?;
      let mut decoded = Vec::new();
      let mut total = 0;
      for image in images {
        let bytes = match image {
          ImageSource::Base64(data) => crate::utils::blocking::cpu(move || {
            base64::engine::general_purpose::STANDARD.decode(&data)
          })
          .await
          .map_err(|e| e.to_string())?
          .map_err(|_| "invalid generated image encoding".to_string())?,
          ImageSource::Url(url) => {
            crate::extensions::shared::artifacts::download(
              &service.transport,
              &url,
              super::identity::WISH_USER_AGENT,
              execution.base_url,
            )
            .await?
          }
        };
        total += bytes.len();
        if total > 64 * 1024 * 1024 || bytes.len() > 32 * 1024 * 1024 {
          return Err("generated images exceed storage limit".into());
        }
        if crate::utils::image_format::supported_mime(&bytes).is_none() {
          return Err("provider returned an unsupported image format".into());
        }
        decoded.push(bytes);
      }
      let mut stored = Vec::new();
      for bytes in decoded {
        stored.push(blobs::save_upload(directory, &bytes).await.map_err(|e| e.to_string())?);
      }
      Ok::<Value, String>(json!({"provider":id,"model":service.config.model,"images":stored}))
    };
    match tokio::time::timeout(Duration::from_secs(600), job).await {
      Ok(Ok(output)) => Ok(output),
      Ok(Err(error)) if !submitted.load(std::sync::atomic::Ordering::Relaxed) => Err(error.into()),
      Ok(Err(error)) => Err(crate::tool::paid_operation::Failure::Unknown(error)),
      Err(_) => Err(if submitted.load(std::sync::atomic::Ordering::Relaxed) {
        crate::tool::paid_operation::Failure::Unknown("Image generation timed out".into())
      } else {
        crate::tool::paid_operation::Failure::from("Image preparation timed out".to_string())
      }),
    }
  }
}
/// Read bounded, session-owned inputs; paths and remote URLs are not accepted.
async fn load_references(
  directory: &Path,
  ids: &[String],
) -> Result<Vec<wire::ReferenceInput>, String> {
  use tokio::io::AsyncReadExt;
  if ids.is_empty() {
    return Ok(Vec::new());
  }
  let root = if ids.iter().any(|id| !id.starts_with("https://")) {
    Some(tokio::fs::canonicalize(directory).await.map_err(|e| e.to_string())?)
  } else {
    None
  };
  let mut result = Vec::new();
  let mut remaining = 12 * 1024 * 1024u64;
  for id in ids {
    if id.starts_with("https://") {
      let url = crate::extensions::shared::urls::validate_url(id).map_err(|e| e.to_string())?;
      if id.len() > 8192 || url.fragment().is_some() {
        return Err("reference URL is too long or contains a fragment".into());
      }
      result.push(wire::ReferenceInput::Url(id.clone()));
      continue;
    }
    let root = root.as_ref().ok_or("missing blob directory")?;
    if !blobs::is_blob_id(id) {
      return Err("reference image must be a session blob ID".into());
    }
    let path =
      tokio::fs::canonicalize(root.join(id)).await.map_err(|_| "reference image blob not found")?;
    if path.parent() != Some(root.as_path()) {
      return Err("reference image must belong to this session".into());
    }
    let file = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
    if file.metadata().await.map_err(|e| e.to_string())?.len() > remaining {
      return Err("reference images exceed the combined 12 MiB limit".into());
    }
    let mut bytes = Vec::new();
    file.take(remaining + 1).read_to_end(&mut bytes).await.map_err(|e| e.to_string())?;
    if bytes.len() as u64 > remaining {
      return Err("reference images exceed the combined 12 MiB limit".into());
    }
    remaining -= bytes.len() as u64;
    let image = crate::utils::blocking::cpu(move || {
      let mime = crate::utils::image_format::supported_mime(&bytes)
        .ok_or("unsupported reference image format")?;
      if !matches!(mime, "image/png" | "image/jpeg" | "image/webp") {
        return Err("reference images must be PNG, JPEG or WebP");
      }
      Ok(wire::ReferenceImage {
        mime: mime.into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
      })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(str::to_owned)?;
    result.push(wire::ReferenceInput::Inline(image));
  }
  Ok(result)
}
