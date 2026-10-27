//! Account credential persistence and scheduled renewal shared by sign-in providers.
use super::{app::App, error::ApplicationError};
use crate::{protocol::endpoint::Credentials, utils::time::unix_seconds};
use std::{sync::Arc, time::Duration};
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
    if !matches!(account.preset.as_deref(), Some("openai_codex" | "github_copilot")) {
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
    let value = serde_json::to_value(next).map_err(ApplicationError::internal)?;
    match app.save_config(revision, value).await {
      Ok(_) => return Ok(()),
      Err(error) if error.kind == crate::application::error::ErrorKind::Conflict => continue,
      Err(error) => return Err(error),
    }
  }
  Err(ApplicationError::conflict("configuration kept changing during sign-in"))
}

pub fn start_refresh_worker(app: &Arc<App>) {
  let worker = Arc::clone(app);
  app.lifecycle.tasks.spawn(async move {
    loop {
      refresh_due(&worker).await;
      tokio::select! {
        _ = worker.lifecycle.stop.cancelled() => break,
        _ = tokio::time::sleep(Duration::from_secs(60)) => {},
      }
    }
  });
}

async fn refresh_due(app: &Arc<App>) {
  let now = unix_seconds();
  let due: Vec<_> = app
    .accounts
    .read()
    .unwrap()
    .clone()
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
    match account.client.refresh_credentials().await {
      Ok(tokens) => {
        if let Err(error) =
          save_credentials_if_current(app, id, &tokens, Some(&account.config)).await
        {
          eprintln!("Token refresh for {id} could not be saved: {error}");
        }
      }
      Err(error) => eprintln!("Token refresh for {id} failed: {error}"),
    }
  }
}
