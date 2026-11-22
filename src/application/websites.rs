//! Live website resources store an address on the development host, never a copy of its pages.
use super::{
  app::App,
  attachments,
  error::{ApplicationError, blocking},
};
use crate::storage::blobs;
use reqwest::Url;
use serde::Deserialize;
use std::sync::Arc;
use tokio::io::AsyncReadExt;

use blobs::{WEBSITE_MIME, WEBSITE_PREFIX};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Website {
  pub url: String,
  pub name: Option<String>,
  pub parent: Option<String>,
}

fn address(raw: &str) -> Result<Url, ApplicationError> {
  let url = Url::parse(raw.trim())
    .map_err(|_| ApplicationError::invalid("Enter a local HTTP or HTTPS URL"))?;
  let local = match url.host() {
    Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
    Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
    Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
    None => false,
  };
  if !local
    || !matches!(url.scheme(), "http" | "https")
    || !url.username().is_empty()
    || url.password().is_some()
    || raw.len() > 8192
  {
    return Err(ApplicationError::invalid("Use an HTTP(S) URL on localhost or a loopback address"));
  }
  Ok(url)
}
pub async fn create(
  app: &Arc<App>,
  owner: &str,
  input: Website,
) -> Result<blobs::Blob, ApplicationError> {
  let url = address(&input.url)?;
  let name = blobs::library::display_name(
    input
      .name
      .as_deref()
      .map(str::trim)
      .filter(|s| !s.is_empty())
      .unwrap_or(&url[url::Position::BeforeHost..url::Position::AfterPort]),
  )?;
  attachments::require_owner(app, owner).await?;
  let slot = app.get_session(owner).await?;
  let guard = slot.blob_publication.clone().read_owned().await;
  blocking(move || {
    let _guard = guard;
    slot.require_live()?;
    let bytes = format!("{WEBSITE_PREFIX}{}\n{}", uuid::Uuid::new_v4(), url);
    let blob = blobs::save_website(&slot.blob_dir, bytes.as_bytes())?;
    let publish = || -> Result<(), ApplicationError> {
      slot.aliases.blob(&blob.id)?;
      blobs::library::record_in_folder(&slot.blob_dir, &blob.id, Some(name), input.parent)?;
      Ok(())
    };
    if let Err(error) = publish() {
      blobs::library::delete(&slot.blob_dir, &blob.id)?;
      return Err(error);
    }
    Ok(blob)
  })
  .await
}

pub struct WebsiteSource {
  pub url: Url,
  entry: std::path::PathBuf,
}
impl WebsiteSource {
  pub async fn available(&self) -> bool {
    tokio::fs::try_exists(&self.entry).await.unwrap_or(false)
  }
}
pub async fn read(
  app: &Arc<App>,
  owner: &str,
  id: &str,
) -> Result<WebsiteSource, ApplicationError> {
  let source = attachments::open(app, owner, id).await?;
  if source.mime_type != WEBSITE_MIME || source.length > 8300 {
    return Err(ApplicationError::invalid("Not a website resource"));
  }
  let mut text = String::new();
  source.file.take(8301).read_to_string(&mut text).await.map_err(ApplicationError::internal)?;
  let (key, raw) = text
    .strip_prefix(WEBSITE_PREFIX)
    .and_then(|s| s.split_once('\n'))
    .ok_or_else(|| ApplicationError::invalid("Invalid website resource"))?;
  uuid::Uuid::parse_str(key).map_err(|_| ApplicationError::invalid("Invalid website resource"))?;
  Ok(WebsiteSource {
    url: address(raw)?,
    entry: blobs::description_path(&app.data_dir.blobs(owner), id),
  })
}

use std::{
  collections::HashMap,
  sync::Mutex,
  time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

pub(crate) struct Preview {
  pub source: WebsiteSource,
  pub secret: String,
  owner: String,
  blob: String,
  stop: CancellationToken,
  expires: Instant,
  pub secure_cookies: std::sync::atomic::AtomicBool,
}

impl Preview {
  pub async fn closed(&self) {
    tokio::select! {
      _ = self.stop.cancelled() => {},
      _ = tokio::time::sleep_until(self.expires.into()) => {},
    }
  }
}

#[derive(Default)]
pub(crate) struct Previews(Mutex<HashMap<String, Arc<Preview>>>);
impl Previews {
  fn grants(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Preview>>> {
    self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
  }
  pub async fn issue(
    app: &Arc<App>,
    owner: String,
    blob: String,
  ) -> Result<(String, String), ApplicationError> {
    app.lifecycle.require_open()?;
    let slot = app.get_session(&owner).await?;
    let _publication = slot.blob_publication.read().await;
    slot.require_live()?;
    let source = read(app, &owner, &blob).await?;
    let label = format!("ws-{}", uuid::Uuid::new_v4().simple());
    let secret = uuid::Uuid::new_v4().simple().to_string();
    app
      .lifecycle
      .while_open(|| {
        let mut grants = app.previews.grants();
        if grants.len() >= 512 {
          return Err(ApplicationError::conflict("Too many active website previews"));
        }
        grants.insert(
          label.clone(),
          Arc::new(Preview {
            source,
            secret: secret.clone(),
            owner,
            blob,
            stop: app.lifecycle.stop.child_token(),
            secure_cookies: false.into(),
            expires: Instant::now() + Duration::from_secs(3600),
          }),
        );
        Ok((label, secret))
      })
      .ok_or_else(ApplicationError::shutting_down)?
  }
  pub(crate) fn get(&self, label: &str) -> Option<Arc<Preview>> {
    self
      .grants()
      .get(label)
      .filter(|g| g.expires > Instant::now() && !g.stop.is_cancelled())
      .cloned()
  }
  pub(crate) fn close(&self, owner: &str, blob: &str, label: &str) {
    let mut grants = self.grants();
    if grants.get(label).is_some_and(|g| g.owner == owner && g.blob == blob)
      && let Some(grant) = grants.remove(label)
    {
      grant.stop.cancel();
    }
  }
  pub(super) fn revoke(&self, owner: &str, blob: Option<&str>) {
    self.grants().retain(|_, grant| {
      if grant.owner == owner && blob.is_none_or(|id| id == grant.blob) {
        grant.stop.cancel();
        false
      } else {
        true
      }
    });
  }
  pub(crate) fn discard(&self, label: &str) {
    if let Some(grant) = self.grants().remove(label) {
      grant.stop.cancel();
    }
  }
  pub(super) async fn reap(&self) {
    let entries = self.grants().clone();
    for (label, grant) in entries {
      if grant.expires <= Instant::now() || !grant.source.available().await {
        self.discard(&label);
      }
    }
  }
}
