//! Provider-owned resources that can be referenced by a stateless generation request.
//! Each authenticated client owns a bounded cache. Keys also include the session.
mod files;
mod prefix;
use super::{Client, image_payload::Images};
use crate::protocol::model_use::resources::{FileProtocol, PrefixProtocol, Resource};
use crate::protocol::{attempt::Transport, endpoint::Draft, error::Error};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
  num::NonZeroUsize,
  sync::{Arc, Mutex},
  time::{Duration, Instant},
};
use tokio::sync::OnceCell;

type FileCell = Arc<OnceCell<(Instant, Option<String>)>>;
#[derive(Clone)]
struct Prefix {
  digests: Vec<[u8; 32]>,
  settings: [u8; 32],
  id: Option<String>,
  until: Instant,
}
pub(super) struct Cache {
  files: Mutex<lru::LruCache<(String, [u8; 32]), FileCell>>,
  prefixes: Mutex<lru::LruCache<(String, String), Prefix>>,
}
impl Default for Cache {
  fn default() -> Self {
    Self {
      files: Mutex::new(lru::LruCache::new(NonZeroUsize::new(512).unwrap())),
      prefixes: Mutex::new(lru::LruCache::new(NonZeroUsize::new(128).unwrap())),
    }
  }
}
impl<T: Transport + Send + Sync> Client<T> {
  pub(super) async fn resource_json(&self, draft: Draft) -> Result<Value, Error> {
    let call =
      self.endpoint.build_call(draft, &self.credentials, crate::utils::time::unix_seconds())?;
    log_transfer("resource", call.body.len(), 0);
    let (_, value) = self.read_reply(self.transport.execute(&call).await?)?;
    Ok(value)
  }
  pub(super) async fn apply_resources(
    &self,
    body: &mut Value,
    images: &Images<'_>,
    model: &str,
  ) -> Result<(), Error> {
    let Some(resources) = &self.resources else { return Ok(()) };
    let Some(session) = self.endpoint.session_id() else { return Ok(()) };
    if !resources.image_locations.is_empty() {
      use base64::Engine;
      for (token, encoded, mime) in images.bindings().filter(|(_, e, _)| !e.is_empty()) {
        let bytes = base64::engine::general_purpose::STANDARD
          .decode(encoded)
          .map_err(|_| Error::Build("invalid image encoding".into()))?;
        if let Some(uri) = resources.image_locations.get(&hex::encode(Sha256::digest(bytes))) {
          files::object_location(body, token, mime, uri);
        }
      }
    }
    if let Some(files) = &resources.files {
      self.apply_files(body, images, session, files).await?;
    }
    if let Some(prefix) = &resources.prefix
      && body.get("previous_response_id").is_none()
    {
      self.apply_prefix(body, images, session, model, prefix).await?;
    }
    Ok(())
  }
}

impl Cache {
  pub(super) fn forget(&self, session: &str, model: &str) {
    self.prefixes.lock().unwrap().pop(&(session.to_owned(), model.to_owned()));
    let mut files = self.files.lock().unwrap();
    let keys: Vec<_> =
      files.iter().filter(|((s, _), _)| s == session).map(|(key, _)| key.clone()).collect();
    for key in keys {
      files.pop(&key);
    }
  }
}
impl<T: Transport + Send + Sync> Client<T> {
  pub(super) fn resource_rejected(&self, error: &Error) -> bool {
    let Error::Upstream { status: Some(400 | 403 | 404 | 410 | 422), code, message, .. } = error
    else {
      return false;
    };
    let text = format!("{code:?} {message}").to_ascii_lowercase();
    let resource =
      ["file", "cachedcontent", "cached content", "cache id", "s3location", "oss://", "gs://"]
        .iter()
        .any(|s| text.contains(s))
        || (self
          .resources
          .as_ref()
          .is_some_and(|r| r.prefix.as_ref().is_some_and(|p| p.protocol == PrefixProtocol::Ark))
          && text.contains("previous_response"));
    resource
      && [
        "invalid",
        "expired",
        "not found",
        "not_found",
        "does not exist",
        "permission",
        "not supported",
      ]
      .iter()
      .any(|s| text.contains(s))
  }
}
pub(super) fn log_transfer(kind: &str, bytes: usize, image_base64_bytes: usize) {
  static ENABLED: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| std::env::var("WISH_MODEL_TRANSFER_LOG").as_deref() == Ok("1"));
  if *ENABLED {
    eprintln!(
      "model_transfer kind={kind} body_bytes={bytes} image_base64_bytes={image_base64_bytes}"
    );
  }
}
