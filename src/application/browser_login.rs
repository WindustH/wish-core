//! Login state, token persistence and renewal. HTTP callbacks are server adapters.
use crate::application::{
  app::App,
  credentials::{LoginStatus, save_credentials},
  error::ApplicationError,
};
use crate::protocol::endpoint::{Credentials, codex_oauth, google_oauth};
use crate::transport::{Proxy, ReqwestTransport};
use serde_json::Value;
use serde_json::json;
use std::{
  sync::Arc,
  time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Browser grants share state, PKCE and persistence; the issuer owns the token wire.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LoginKind {
  Chatgpt,
  Google,
}
impl LoginKind {
  fn check(self, app: &App, provider: &str) -> Result<(), ApplicationError> {
    let account = app.get_login_account(provider)?;
    let preset = match self {
      Self::Chatgpt => "openai_codex",
      Self::Google => "google_antigravity",
    };
    if account.config.preset.as_deref() != Some(preset) {
      return Err(ApplicationError::invalid(format!(
        "this login requires the {preset} account preset"
      )));
    }
    Ok(())
  }
}

#[derive(Default)]
pub struct LoginManager {
  pub(crate) attempt: Mutex<Option<LoginAttempt>>,
}

pub(crate) struct LoginAttempt {
  pub(crate) id: String,
  pub(crate) provider: String,
  pub(crate) expires: Instant,
  pub(crate) status: LoginStatus,
  pub(crate) error: Option<String>,
  pub(crate) grant: LoginGrant,
}

#[derive(Clone)]
pub(crate) struct LoginGrant {
  pub(crate) state: String,
  pub(crate) verifier: String,
  pub(crate) redirect_uri: String,
  pub(crate) issuer: String,
  pub(crate) kind: LoginKind,
  pub(crate) proxy: Proxy,
  pub(crate) end: CancellationToken,
}

impl LoginManager {
  pub(crate) async fn claim(&self, id: &str) -> bool {
    let mut attempt = self.attempt.lock().await;
    let Some(current) = attempt.as_mut().filter(|a| a.id == id && a.status == LoginStatus::Pending)
    else {
      return false;
    };
    if current.expires <= Instant::now() {
      current.status = LoginStatus::Expired;
      return false;
    }
    current.status = LoginStatus::Processing;
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
      Ok(credentials) => self.persist_if_active(app, id, provider, grant.kind, &credentials).await,
      Err(error) => Err(error),
    };
    match &result {
      Ok(()) => self.finish(id, LoginStatus::Complete, None).await,
      Err(error) => self.finish(id, LoginStatus::Failed, Some(error.to_string())).await,
    }
    grant.end.cancel();
    result
  }

  async fn persist_if_active(
    &self,
    app: &App,
    id: &str,
    provider: &str,
    kind: LoginKind,
    credentials: &Credentials,
  ) -> Result<(), ApplicationError> {
    let attempt = self.attempt.lock().await;
    if !attempt.as_ref().is_some_and(|a| a.id == id && a.status == LoginStatus::Processing) {
      return Err(ApplicationError::conflict("login attempt was replaced"));
    }
    kind.check(app, provider)?;
    save_credentials(app, provider, credentials).await
  }

  pub(crate) async fn finish(&self, id: &str, status: LoginStatus, error: Option<String>) {
    let mut attempt = self.attempt.lock().await;
    if let Some(current) = attempt.as_mut().filter(|a| {
      a.id == id && (a.status == LoginStatus::Pending || a.status == LoginStatus::Processing)
    }) {
      current.status = status;
      current.error = error;
    }
  }
}

async fn exchange_code(grant: &LoginGrant, code: &str) -> Result<Credentials, ApplicationError> {
  let transport = ReqwestTransport::new(Default::default(), grant.proxy.clone())?;
  if grant.kind == LoginKind::Google {
    let tokens =
      google_oauth::exchange_code(&transport, code, &grant.redirect_uri, &grant.verifier).await?;
    if tokens.refresh_token.is_none() {
      return Err(ApplicationError::invalid(
        "Google login did not return a refresh token; grant offline access again",
      ));
    }
    let mut credentials = Credentials::default().renew(&tokens);
    credentials.project =
      Some(super::google_login::discover_project(&transport, &credentials, &grant.end).await?);
    return Ok(credentials);
  }
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
  Ok(Credentials::default().renew(&tokens))
}

pub const LOGIN_LIFETIME: Duration = Duration::from_secs(600);
pub async fn status(
  app: Arc<App>,
  provider_id: String,
  kind: LoginKind,
) -> Result<Value, ApplicationError> {
  kind.check(&app, &provider_id)?;
  let attempt = app.browser_login.attempt.lock().await;
  let Some(attempt) =
    attempt.as_ref().filter(|a| a.provider == provider_id && a.grant.kind == kind)
  else {
    return Ok(json!({"status":"idle"}));
  };
  Ok(json!({"status":attempt.status,"error":attempt.error}))
}
pub async fn complete(
  app: Arc<App>,
  provider_id: String,
  callback_url: String,
  kind: LoginKind,
) -> Result<Value, ApplicationError> {
  app.lifecycle.require_open()?;
  let (id, grant) = {
    let attempt = app.browser_login.attempt.lock().await;
    let current =
      attempt.as_ref().filter(|a| a.provider == provider_id && a.grant.kind == kind).ok_or_else(
        || ApplicationError::conflict("start a browser login before submitting its redirect URL"),
      )?;
    if current.status == LoginStatus::Complete {
      return Ok(json!({"status":"complete"}));
    }
    if current.status != LoginStatus::Pending || current.expires <= Instant::now() {
      return Err(ApplicationError::conflict(
        "Browser login expired or is no longer pending; start again",
      ));
    }
    (current.id.clone(), current.grant.clone())
  };
  let url = reqwest::Url::parse(callback_url.trim())
    .map_err(|_| ApplicationError::invalid("paste the complete redirect URL from the browser"))?;
  let mut base = url.clone();
  base.set_query(None);
  base.set_fragment(None);
  let expected = reqwest::Url::parse(&grant.redirect_uri).map_err(ApplicationError::internal)?;
  if base != expected {
    return Err(ApplicationError::invalid("redirect URL does not match this login attempt"));
  }
  let mut state = None;
  let mut code = None;
  for (name, value) in url.query_pairs() {
    let (seen, name) = match name.as_ref() {
      "state" => (&mut state, "state"),
      "code" => (&mut code, "code"),
      _ => continue,
    };
    if seen.replace(value.into_owned()).is_some() {
      return Err(ApplicationError::invalid(format!("redirect URL has duplicate {name}")));
    }
  }
  if state.as_deref() != Some(grant.state.as_str()) {
    return Err(ApplicationError::invalid("authorization state did not match"));
  }
  let code = code
    .filter(|value| !value.is_empty())
    .ok_or_else(|| ApplicationError::invalid("redirect URL contains no authorization code"))?;
  if !app.browser_login.claim(&id).await {
    return Err(ApplicationError::conflict("login attempt is no longer pending"));
  }
  app.browser_login.redeem(&app, &id, &grant, &provider_id, &code).await?;
  Ok(json!({"status":"complete"}))
}
/// Registers a login while serializing replacement of its callback endpoint.
/// The server supplies endpoint binding; this layer owns account checks, PKCE and expiry.
pub struct PendingLogin<T> {
  pub callback_endpoint: T,
  pub attempt_id: String,
  pub grant: LoginGrant,
  pub authorization_url: String,
}
pub async fn begin<T>(
  app: &Arc<App>,
  provider_id: &str,
  kind: LoginKind,
  callback_endpoint: impl std::future::Future<Output = Result<(T, String), ApplicationError>>,
) -> Result<PendingLogin<T>, ApplicationError> {
  app.lifecycle.require_open()?;
  kind.check(app, provider_id)?;
  let account = app.get_login_account(provider_id)?;
  let proxy = app.config_file.lock().await.config.proxy.policy(account.config.proxy_enabled);
  let mut current = app.browser_login.attempt.lock().await;
  if let Some(previous) = current.take() {
    previous.grant.end.cancel();
  }
  let (callback_endpoint, redirect_uri) = callback_endpoint.await?;
  let state = uuid::Uuid::new_v4().to_string();
  let verifier = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
  let issuer = codex_oauth::issuer();
  let url = match kind {
    LoginKind::Chatgpt => {
      codex_oauth::authorization_url(&issuer, &redirect_uri, &state, &verifier)?.to_string()
    }
    LoginKind::Google => google_oauth::authorization_url(&redirect_uri, &state, &verifier)?,
  };
  let attempt_id = uuid::Uuid::new_v4().to_string();
  let end = CancellationToken::new();
  let grant = LoginGrant { state, verifier, redirect_uri, issuer, kind, proxy, end: end.clone() };
  *current = Some(LoginAttempt {
    id: attempt_id.clone(),
    provider: provider_id.to_owned(),
    expires: Instant::now() + LOGIN_LIFETIME,
    status: LoginStatus::Pending,
    error: None,
    grant: grant.clone(),
  });
  drop(current);
  Ok(PendingLogin { callback_endpoint, attempt_id, grant, authorization_url: url.to_string() })
}

/// Browser callback failures safe to describe without exposing the upstream token exchange.
pub enum CallbackFailure {
  StateMismatch,
  Declined,
  MissingCode,
  Inactive,
  RedemptionFailed,
}
pub async fn receive_callback(
  app: &Arc<App>,
  attempt_id: &str,
  grant: &LoginGrant,
  provider_id: &str,
  state: Option<&str>,
  code: Option<&str>,
  error: Option<&str>,
) -> Result<(), CallbackFailure> {
  if state != Some(grant.state.as_str()) {
    return Err(CallbackFailure::StateMismatch);
  }
  if error.is_some() {
    app
      .browser_login
      .finish(attempt_id, LoginStatus::Failed, Some("Authorization was declined".to_owned()))
      .await;
    grant.end.cancel();
    return Err(CallbackFailure::Declined);
  }
  let code = code.filter(|code| !code.is_empty()).ok_or(CallbackFailure::MissingCode)?;
  if !app.browser_login.claim(attempt_id).await {
    return Err(CallbackFailure::Inactive);
  }
  app
    .browser_login
    .redeem(app, attempt_id, grant, provider_id, code)
    .await
    .map_err(|_| CallbackFailure::RedemptionFailed)
}
