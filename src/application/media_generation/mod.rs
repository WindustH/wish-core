//! Durable media submissions. Status resumes polling/downloads; it never resubmits a paid job.
use super::{
  accounts::{AccountConfig, AccountRegistry},
  config::ProxyConfig,
  error::ApplicationError,
};
use crate::{
  protocol::{
    attempt::{Call, Limits, Method, ReplyStream, Transport},
    endpoint::Draft,
    media_generation::{MediaKind, MediaOptions, MediaProtocol},
  },
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
mod jobs;
use config::preset;
pub use config::{MediaConfig, MediaProvider, presets};
pub use jobs::Request;

pub(super) struct State {
  pub(super) config: MediaConfig,
  pub(super) proxy: ProxyConfig,
}
pub struct PreparedMedia(State);
pub struct MediaHub {
  pub(super) state: RwLock<Arc<State>>,
  pub(super) permits: tokio::sync::Semaphore,
  // Bounded striped locks prevent two status/run requests from mutating the same job.
  locks: Vec<tokio::sync::Mutex<()>>,
}
impl MediaHub {
  pub fn new(config: &MediaConfig, proxy: &ProxyConfig) -> Self {
    Self {
      state: RwLock::new(Arc::new(State { config: config.clone(), proxy: proxy.clone() })),
      permits: tokio::sync::Semaphore::new(2),
      locks: (0..64).map(|_| tokio::sync::Mutex::new(())).collect(),
    }
  }
  pub fn prepare(config: &MediaConfig, proxy: &ProxyConfig) -> PreparedMedia {
    PreparedMedia(State { config: config.clone(), proxy: proxy.clone() })
  }
  pub fn apply(&self, prepared: PreparedMedia) {
    *self.state.write().unwrap() = Arc::new(prepared.0);
  }
  pub fn provider_catalog(&self, accounts: &AccountRegistry) -> Catalog {
    let state = self.state.read().unwrap();
    Catalog {
      default_order: state.config.order.clone(),
      failure_policy: "first_enabled_compatible_kind_only_no_paid_submission_failover",
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
            enabled: p.enabled,
            available: p.enabled
              && accounts
                .get(&format!("account:{}", p.credential_id))
                .is_some_and(|a| !a.client.get_credentials().api_key.is_empty()),
          })
        })
        .collect(),
    }
  }
}
