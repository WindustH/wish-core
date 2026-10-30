//! Account credential persistence and scheduled renewal shared by sign-in providers.
use super::{app::App, error::ApplicationError};
use crate::{protocol::endpoint::Credentials, utils::time::unix_seconds};
use std::{
  collections::BTreeMap,
  sync::Arc,
  time::{Duration, Instant},
};
const REFRESH_MARGIN: u64 = 300;
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginStatus {
  Pending,
  Processing,
  Complete,
  Expired,
  Failed,
}
/// Replaces one subscription account's OAuth material through a configuration save, without it
/// passing through the configuration API. When another save lands first, it tries again on top of
/// that one.
pub(crate) async fn save_credentials(
  app: &App,
  id: &str,
  credentials: &Credentials,
) -> Result<(), ApplicationError> {
  save_credentials_if_current(app, id, credentials, None).await
}

async fn save_credentials_if_current(
  app: &App,
  id: &str,
  credentials: &Credentials,
  expected: Option<&super::accounts::AccountConfig>,
) -> Result<(), ApplicationError> {
  for _ in 0..3 {
    let (revision, mut next) = {
      let current = app.config_file.lock().await;
      (current.revision.clone(), current.config.clone())
    };
    let account_id = id
      .strip_prefix("account:")
      .map(str::to_owned)
      .or_else(|| next.providers.get(id).and_then(|p| p.credential_id.clone()))
      .ok_or_else(ApplicationError::not_found)?;
    let account = next.accounts.get_mut(&account_id).ok_or_else(ApplicationError::not_found)?;
    if account.get_renewal().is_none() {
      return Err(ApplicationError::invalid("account does not support token renewal"));
    }
    if let Some(expected) = expected
      && (account.api_key != expected.api_key
        || account.refresh_token != expected.refresh_token
        || account.preset != expected.preset)
    {
      return Ok(()); // A new login or account edit supersedes this background renewal.
    }
    account.api_key = Some(credentials.api_key.clone());
    account.api_key_env = None;
    if let Some(token) = &credentials.refresh_token {
      account.refresh_token = Some(token.clone());
    }
    account.expires_at = credentials.expires_at;
    if let Some(account_id) = &credentials.account_id {
      account.credentials.insert("account_id".to_owned(), account_id.clone());
      account.credentials_env.remove("account_id");
    }
    if let Some(project) = &credentials.project {
      account.credentials.insert("project".to_owned(), project.clone());
      account.credentials_env.remove("project");
    }
    let value = serde_json::to_value(next).map_err(ApplicationError::internal)?;
    match app.save_config(revision, value).await {
      Ok(_) => return Ok(()),
      Err(error) if error.kind == crate::application::error::ErrorKind::Conflict => continue,
      Err(error) => return Err(error),
    }
  }
  Err(ApplicationError::conflict("configuration kept changing during sign-in"))
}

/// Failures are scoped to the credential/configuration revision. Unrelated config saves
/// keep the delay; editing the account or logging in again permits an immediate attempt.
struct RefreshFailure {
  config: super::accounts::AccountConfig,
  failures: u32,
  retry_at: Instant,
}
#[derive(Default)]
struct RefreshSchedule(BTreeMap<String, RefreshFailure>);
impl RefreshSchedule {
  fn ready(&mut self, id: &str, config: &super::accounts::AccountConfig, now: Instant) -> bool {
    match self.0.get(id) {
      Some(failure) if failure.config == *config => now >= failure.retry_at,
      _ => {
        self.0.remove(id);
        true
      }
    }
  }
  fn failed(&mut self, id: &str, config: &super::accounts::AccountConfig, now: Instant) {
    let failures = self
      .0
      .get(id)
      .filter(|previous| previous.config == *config)
      .map_or(1, |previous| previous.failures.saturating_add(1));
    let delay = Duration::from_secs((60u64.saturating_mul(1 << failures.min(6))).min(3600));
    self.0.insert(
      id.into(),
      RefreshFailure { config: config.clone(), failures, retry_at: now + delay },
    );
  }
}

pub fn start_refresh_worker(app: &Arc<App>) {
  let worker = Arc::clone(app);
  app.lifecycle.tasks.spawn(async move {
    let mut schedule = RefreshSchedule::default();
    loop {
      tokio::select! {
        _ = worker.lifecycle.stop.cancelled() => break,
        _ = refresh_due(&worker, &mut schedule) => {}
      }
      tokio::select! {
        _ = worker.lifecycle.stop.cancelled() => break,
        _ = tokio::time::sleep(Duration::from_secs(60)) => {},
      }
    }
  });
}

async fn refresh_due(app: &Arc<App>, schedule: &mut RefreshSchedule) {
  let now = unix_seconds();
  let accounts = app.get_service_accounts();
  schedule.0.retain(|id, _| accounts.contains_key(id));
  let due: Vec<_> = accounts
    .into_values()
    .filter(|account| {
      account.enabled
        && account.config.get_renewal().is_some()
        && account.config.refresh_token.as_deref().is_some_and(|token| !token.is_empty())
        && account.config.expires_at.is_some_and(|expires| expires <= now + REFRESH_MARGIN)
    })
    .collect();
  for account in due {
    let id = &account.id;
    if !schedule.ready(id, &account.config, Instant::now()) {
      continue;
    }
    match account.client.refresh_credentials().await {
      Ok(tokens) => {
        if let Err(error) =
          save_credentials_if_current(app, id, &tokens, Some(&account.config)).await
        {
          schedule.failed(id, &account.config, Instant::now());
          eprintln!("Token refresh for {id} could not be saved: {error}");
        } else {
          schedule.0.remove(id);
        }
      }
      Err(error) => {
        schedule.failed(id, &account.config, Instant::now());
        eprintln!("Token refresh for {id} failed; automatic renewal backed off: {error}");
      }
    }
  }
}
