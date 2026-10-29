//! Durable media submissions. Status resumes polling/downloads; it never resubmits a paid job.
use super::{
  accounts::{AccountConfig, AccountRegistry},
  config::ProxyConfig,
  error::ApplicationError,
};
use crate::extension_host::media::{MediaKind, MediaProtocol, Parameters};
use crate::{
  protocol::attempt::{Limits, ReplyStream, Transport},
  storage::{blobs, media},
  tool::providers::{Catalog, Provider},
  transport::ReqwestTransport,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
  collections::BTreeMap,
  path::Path,
  sync::{Arc, RwLock},
  time::Duration,
};

mod config;
mod inputs;
mod jobs;
mod preflight;
use config::preset;
pub use config::{MediaConfig, MediaProvider, presets};
pub use jobs::{Request, Scope};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum TransferProfile {
  VideoSubmission,
  AudioSubmission,
  TimedSpeech,
  Status,
  Artifact,
  Transcription,
  Upload,
}

pub(super) struct State {
  pub(super) config: MediaConfig,
  pub(super) proxy: ProxyConfig,
  transports: std::sync::Mutex<BTreeMap<(bool, TransferProfile), ReqwestTransport>>,
}
pub struct PreparedMedia(State);
pub struct MediaHub {
  pub(super) state: RwLock<Arc<State>>,
  pub(super) permits: tokio::sync::Semaphore,
  pub(super) status_permits: tokio::sync::Semaphore,
  pub(super) download_permits: tokio::sync::Semaphore,
  // Only live requests retain a per-job lock; unrelated jobs never share a lock.
  locks: crate::extensions::shared::jobs::JobLocks,
}
impl MediaHub {
  pub fn new(config: &MediaConfig, proxy: &ProxyConfig) -> Self {
    Self {
      state: RwLock::new(Arc::new(State {
        config: config.clone(),
        proxy: proxy.clone(),
        transports: Default::default(),
      })),
      permits: tokio::sync::Semaphore::new(2),
      status_permits: tokio::sync::Semaphore::new(4),
      download_permits: tokio::sync::Semaphore::new(2),
      locks: Default::default(),
    }
  }
  pub fn prepare(config: &MediaConfig, proxy: &ProxyConfig) -> PreparedMedia {
    PreparedMedia(State {
      config: config.clone(),
      proxy: proxy.clone(),
      transports: Default::default(),
    })
  }
  pub fn apply(&self, prepared: PreparedMedia) {
    *self.state.write().unwrap() = Arc::new(prepared.0);
  }
  pub fn provider_catalog(&self, accounts: &AccountRegistry) -> Catalog {
    let state = self.state.read().unwrap();
    Catalog {
      default_order: state.config.order.clone(),
      failure_policy: "first_enabled_compatible_provider_no_paid_submission_failover",
      providers: state
        .config
        .providers
        .iter()
        .filter_map(|(id, p)| {
          let spec = preset(&p.preset).ok()?;
          Some(Provider {
            id: id.clone(),
            name: spec.name.clone(),
            preset: p.preset.clone(),
            model: Some(p.model.clone()),
            kind: Some(spec.kind.as_str().into()),
            reference_images: None,
            mask_editing: None,
            markup_editing: None,
            image_required: None,
            image_options: None,
            precise_references: None,
            video: spec.protocol.video_capabilities(&p.model),
            enabled: p.enabled,
            available: p.enabled
              && accounts
                .get(&format!("account:{}", p.credential_id))
                .is_some_and(|a| spec.protocol.credentials_ready(a.client.get_credentials())),
          })
        })
        .collect(),
    }
  }
}

impl State {
  pub(super) fn transport(
    &self,
    proxy_enabled: bool,
    kind: TransferProfile,
  ) -> Result<ReqwestTransport, String> {
    let mut clients = self.transports.lock().unwrap();
    if let Some(client) = clients.get(&(proxy_enabled, kind)) {
      return Ok(client.clone());
    }
    let limits = match kind {
      TransferProfile::Status => Limits {
        total: Duration::from_secs(60),
        max_response_bytes: 2 * 1024 * 1024,
        ..Default::default()
      },
      TransferProfile::Upload
      | TransferProfile::VideoSubmission
      | TransferProfile::AudioSubmission => Limits {
        total: Duration::from_secs(600),
        first_byte: Duration::from_secs(300),
        max_response_bytes: 2 * 1024 * 1024,
        ..Default::default()
      },
      TransferProfile::Artifact => Limits {
        total: Duration::from_secs(600),
        first_byte: Duration::from_secs(300),
        max_response_bytes: 512 * 1024 * 1024,
        ..Default::default()
      },
      TransferProfile::TimedSpeech => Limits {
        total: Duration::from_secs(600),
        first_byte: Duration::from_secs(300),
        max_response_bytes: 192 * 1024 * 1024,
        ..Default::default()
      },
      TransferProfile::Transcription => Limits {
        total: Duration::from_secs(600),
        first_byte: Duration::from_secs(300),
        max_response_bytes: 16 * 1024 * 1024,
        ..Default::default()
      },
    };
    let client =
      ReqwestTransport::new(limits, self.proxy.policy(proxy_enabled)).map_err(|e| e.to_string())?;
    clients.insert((proxy_enabled, kind), client.clone());
    Ok(client)
  }
}
