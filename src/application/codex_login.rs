//! Login state, token persistence and renewal. HTTP callbacks are server adapters.
use crate::application::{app::App, error::ApplicationError};
use crate::protocol::endpoint::{Credentials, Tokens, codex_oauth};
use crate::transport::{Proxy, ReqwestTransport};
use crate::utils::time::unix_seconds;
use serde_json::Value;
use std::{
  sync::Arc,
  time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
const REFRESH_MARGIN: u64 = 300;
#[derive(Default)]
pub struct LoginManager {
  pub(crate) attempt: Mutex<Option<LoginAttempt>>,
}

pub(crate) struct LoginAttempt {
  pub(crate) id: String,
  pub(crate) provider: String,
  pub(crate) expires: Instant,
  pub(crate) status: &'static str,
  pub(crate) error: Option<String>,
  pub(crate) grant: LoginGrant,
}

#[derive(Clone)]
pub(crate) struct LoginGrant {
  pub(crate) state: String,
  pub(crate) verifier: String,
  pub(crate) redirect_uri: String,
  pub(crate) issuer: String,
  pub(crate) proxy: Proxy,
  pub(crate) end: CancellationToken,
}

impl LoginManager {
  pub(crate) async fn claim(&self, id: &str) -> bool {
    let mut attempt = self.attempt.lock().await;
    let Some(current) = attempt.as_mut().filter(|a| a.id == id && a.status == "pending") else {
      return false;
    };
    if current.expires <= Instant::now() {
      current.status = "expired";
      return false;
    }
    current.status = "processing";
    true
  }

  /// Exchanges the authorization code a claimed attempt brought back for tokens, saves them, and
  /// ends the attempt with how that went.
  pub(crate) async fn redeem(
    &self,
    app: &App,
    id: &str,
    grant: &LoginGrant,
    provider: &str,
    code: &str,
  ) -> Result<(), ApplicationError> {
    let result = match exchange_code(grant, code).await {
      Ok(tokens) => self.persist_if_active(app, id, provider, &tokens).await,
      Err(error) => Err(error),
    };
    match &result {
      Ok(()) => self.finish(id, "complete", None).await,
      Err(error) => self.finish(id, "failed", Some(error.to_string())).await,
    }
    grant.end.cancel();
    result
  }

  async fn persist_if_active(
    &self,
    app: &App,
    id: &str,
    provider: &str,
    tokens: &Tokens,
  ) -> Result<(), ApplicationError> {
    let attempt = self.attempt.lock().await;
    if !attempt.as_ref().is_some_and(|a| a.id == id && a.status == "processing") {
      return Err(ApplicationError::conflict("login attempt was replaced"));
    }
    save_credentials(app, provider, &Credentials::default().renew(tokens)).await
  }

  pub(crate) async fn finish(&self, id: &str, status: &'static str, error: Option<String>) {
    let mut attempt = self.attempt.lock().await;
    if let Some(current) =
      attempt.as_mut().filter(|a| a.id == id && (a.status == "pending" || a.status == "processing"))
    {
      current.status = status;
      current.error = error;
    }
  }
}

async fn exchange_code(grant: &LoginGrant, code: &str) -> Result<Tokens, ApplicationError> {
  let transport = ReqwestTransport::new(Default::default(), grant.proxy.clone())?;
  let reply = codex_oauth::post_authorization_code(
    &transport,
    &grant.issuer,
    code,
    &grant.redirect_uri,
    &grant.verifier,
  )
  .await?;
  if !reply.is_success() {
    return Err(ApplicationError::invalid(format!(
      "ChatGPT token exchange returned HTTP {}",
      reply.status
    )));
  }
  let body: Value = serde_json::from_slice(&reply.body).map_err(ApplicationError::internal)?;
  let tokens = codex_oauth::decode_tokens(&body)?;
  if tokens.account_id.as_deref().unwrap_or_default().is_empty() {
    return Err(ApplicationError::invalid("ChatGPT login did not return a Codex account ID"));
  }
  Ok(tokens)
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
    if let Some(expected) = expected {
      if account.api_key != expected.api_key
        || account.refresh_token != expected.refresh_token
        || account.preset != expected.preset
      {
        return Ok(()); // A new login or account edit supersedes this background renewal.
      }
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
