//! Browser authorization for the Codex subscription preset.

use crate::protocol::outbound::oauth;
use crate::protocol::outbound::{AuthProtocol, Credentials, Draft, Outbound, Tokens};
use crate::protocol::wire::{Method, Transport};
use crate::server::{app::App, error::ApiError};
use crate::transport::{Proxy, ReqwestTransport};
use axum::{
  Json, Router,
  extract::{Path, Query, State},
  http::StatusCode,
  response::{Html, IntoResponse, Response},
  routing::get,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
  collections::HashMap,
  sync::Arc,
  time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const LOGIN_LIFETIME: Duration = Duration::from_secs(600);
const REFRESH_MARGIN: u64 = 300;

#[derive(Default)]
pub struct LoginManager {
  attempt: Mutex<Option<LoginAttempt>>,
}

struct LoginAttempt {
  id: String,
  provider: String,
  expires: Instant,
  status: &'static str,
  error: Option<String>,
  grant: LoginGrant,
}

#[derive(Clone)]
struct LoginGrant {
  state: String,
  verifier: String,
  redirect_uri: String,
  issuer: String,
  proxy: Proxy,
  end: CancellationToken,
}

struct Callback {
  app: Arc<App>,
  attempt_id: String,
  grant: LoginGrant,
  provider: String,
}

#[derive(Deserialize)]
pub struct CallbackUrl {
  callback_url: String,
}

pub async fn start(
  State(app): State<Arc<App>>,
  Path(provider_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  app.require_open()?;
  let provider = app.get_provider(&provider_id)?;
  if provider.config.preset.as_deref() != Some("openai_codex") {
    return Err(ApiError::bad_request("ChatGPT login is available only for the Codex preset"));
  }
  let proxy = {
    let config = app.configuration.lock().await;
    config.config.proxy.policy(provider.config.proxy_enabled)
  };
  let mut current = app.codex_login.attempt.lock().await;
  if let Some(previous) = current.take() {
    previous.grant.end.cancel();
  }
  let listener = bind_callback().await?;
  let port = listener.local_addr().map_err(ApiError::internal)?.port();
  let redirect_uri = format!("http://localhost:{port}/auth/callback");
  let state = uuid::Uuid::new_v4().to_string();
  let verifier = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
  let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
  let issuer = issuer();
  let mut url =
    reqwest::Url::parse(&format!("{issuer}/oauth/authorize")).map_err(ApiError::internal)?;
  url
    .query_pairs_mut()
    .append_pair("response_type", "code")
    .append_pair("client_id", oauth::CLIENT_ID)
    .append_pair("redirect_uri", &redirect_uri)
    .append_pair(
      "scope",
      "openid profile email offline_access api.connectors.read api.connectors.invoke",
    )
    .append_pair("code_challenge", &challenge)
    .append_pair("code_challenge_method", "S256")
    .append_pair("state", &state)
    .append_pair("id_token_add_organizations", "true")
    .append_pair("codex_cli_simplified_flow", "true")
    .append_pair("originator", "codex_cli_rs");
  let attempt_id = uuid::Uuid::new_v4().to_string();
  let end = CancellationToken::new();
  let grant = LoginGrant {
    state,
    verifier,
    redirect_uri,
    issuer,
    proxy,
    end: end.clone(),
  };
  *current = Some(LoginAttempt {
    id: attempt_id.clone(),
    provider: provider_id.clone(),
    expires: Instant::now() + LOGIN_LIFETIME,
    status: "pending",
    error: None,
    grant: grant.clone(),
  });
  drop(current);
  let context = Arc::new(Callback {
    app: Arc::clone(&app),
    attempt_id: attempt_id.clone(),
    grant,
    provider: provider_id,
  });
  let router = Router::new().route("/auth/callback", get(callback)).with_state(context);
  let shutdown = app.stop.clone();
  let worker_app = Arc::clone(&app);
  app.tasks.spawn(async move {
    let _ = axum::serve(listener, router)
      .with_graceful_shutdown(async move {
        tokio::select! {
          _ = end.cancelled() => {},
          _ = shutdown.cancelled() => {},
          _ = tokio::time::sleep(LOGIN_LIFETIME) => {},
        }
      })
      .await;
    worker_app.codex_login.finish(&attempt_id, "expired", None).await;
  });
  Ok(Json(json!({"authorization_url": url.as_str(), "expires_in": LOGIN_LIFETIME.as_secs()})))
}

pub async fn status(
  State(app): State<Arc<App>>,
  Path(provider_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  let provider = app.get_provider(&provider_id)?;
  if provider.config.preset.as_deref() != Some("openai_codex") {
    return Err(ApiError::bad_request("ChatGPT login is available only for the Codex preset"));
  }
  let attempt = app.codex_login.attempt.lock().await;
  let Some(attempt) = attempt.as_ref().filter(|a| a.provider == provider_id) else {
    return Ok(Json(json!({"status":"idle"})));
  };
  Ok(Json(json!({"status":attempt.status,"error":attempt.error})))
}

/// Finish a login when the browser and Wish are on different machines. The pasted
/// redirect carries only a short-lived authorization code; the PKCE verifier stays here.
pub async fn complete(
  State(app): State<Arc<App>>,
  Path(provider_id): Path<String>,
  Json(input): Json<CallbackUrl>,
) -> Result<Json<Value>, ApiError> {
  app.require_open()?;
  let (id, grant) = {
    let attempt = app.codex_login.attempt.lock().await;
    let current = attempt.as_ref().filter(|a| a.provider == provider_id).ok_or_else(|| {
      ApiError::conflict("start a ChatGPT login before submitting its redirect URL")
    })?;
    if current.status == "complete" {
      return Ok(Json(json!({"status":"complete"})));
    }
    if current.status != "pending" || current.expires <= Instant::now() {
      return Err(ApiError::conflict("ChatGPT login expired or is no longer pending; start again"));
    }
    (current.id.clone(), current.grant.clone())
  };
  let url = reqwest::Url::parse(input.callback_url.trim())
    .map_err(|_| ApiError::bad_request("paste the complete redirect URL from the browser"))?;
  let mut base = url.clone();
  base.set_query(None);
  base.set_fragment(None);
  let expected = reqwest::Url::parse(&grant.redirect_uri).map_err(ApiError::internal)?;
  if base != expected {
    return Err(ApiError::bad_request("redirect URL does not match this login attempt"));
  }
  let mut state = None;
  let mut code = None;
  for (name, value) in url.query_pairs() {
    if name == "state" {
      if state.replace(value.into_owned()).is_some() {
        return Err(ApiError::bad_request("redirect URL has duplicate state"));
      }
    } else if name == "code" {
      if code.replace(value.into_owned()).is_some() {
        return Err(ApiError::bad_request("redirect URL has duplicate code"));
      }
    }
  }
  if state.as_deref() != Some(grant.state.as_str()) {
    return Err(ApiError::bad_request("authorization state did not match"));
  }
  let code = code.filter(|value| !value.is_empty()).ok_or_else(|| {
    ApiError::bad_request("redirect URL contains no authorization code")
  })?;
  if !app.codex_login.claim(&id).await {
    return Err(ApiError::conflict("login attempt is no longer pending"));
  }
  let result = match exchange_code(&grant, &code).await {
    Ok(tokens) => app.codex_login.persist_if_active(&app, &id, &provider_id, &tokens).await,
    Err(error) => Err(error),
  };
  grant.end.cancel();
  match result {
    Ok(()) => {
      app.codex_login.finish(&id, "complete", None).await;
      Ok(Json(json!({"status":"complete"})))
    }
    Err(error) => {
      app.codex_login.finish(&id, "failed", Some(error.to_string())).await;
      Err(error)
    }
  }
}

impl LoginManager {
  async fn claim(&self, id: &str) -> bool {
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

  async fn persist_if_active(
    &self,
    app: &App,
    id: &str,
    provider: &str,
    tokens: &Tokens,
  ) -> Result<(), ApiError> {
    let attempt = self.attempt.lock().await;
    if !attempt.as_ref().is_some_and(|a| a.id == id && a.status == "processing") {
      return Err(ApiError::conflict("login attempt was replaced"));
    }
    app.persist_codex_credentials(provider, &Credentials::default().renew(tokens)).await
  }

  async fn finish(&self, id: &str, status: &'static str, error: Option<String>) {
    let mut attempt = self.attempt.lock().await;
    if let Some(current) = attempt.as_mut().filter(|a| a.id == id && (a.status == "pending" || a.status == "processing")) {
      current.status = status;
      current.error = error;
    }
  }
}

async fn bind_callback() -> Result<tokio::net::TcpListener, ApiError> {
  #[cfg(debug_assertions)]
  if let Ok(port) = std::env::var("WISH_TEST_CODEX_CALLBACK_PORT") {
    let port: u16 = port
      .parse()
      .map_err(|error: std::num::ParseIntError| ApiError::bad_request(error.to_string()))?;
    return tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(ApiError::internal);
  }
  for _ in 0..20 {
    for port in [1455, 1457] {
      if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        return Ok(listener);
      }
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
  }
  Err(ApiError::conflict("Codex login callback ports 1455 and 1457 are unavailable"))
}

fn issuer() -> String {
  #[cfg(debug_assertions)]
  if let Ok(value) = std::env::var("WISH_TEST_CODEX_ISSUER") {
    return value.trim_end_matches('/').to_owned();
  }
  oauth::ISSUER.to_owned()
}

async fn callback(
  State(context): State<Arc<Callback>>,
  Query(query): Query<HashMap<String, String>>,
) -> Response {
  if query.get("state") != Some(&context.grant.state) {
    return (StatusCode::BAD_REQUEST, Html("Authorization state did not match")).into_response();
  }
  if let Some(error) = query.get("error") {
    context.app.codex_login.finish(&context.attempt_id, "failed", Some(error.clone())).await;
    context.grant.end.cancel();
    return (StatusCode::BAD_REQUEST, Html("ChatGPT authorization was declined")).into_response();
  }
  let Some(code) = query.get("code").filter(|code| !code.is_empty()) else {
    return (StatusCode::BAD_REQUEST, Html("Authorization code is missing")).into_response();
  };
  if !context.app.codex_login.claim(&context.attempt_id).await {
    return (StatusCode::CONFLICT, Html("Login attempt is no longer active")).into_response();
  }
  let result = match exchange_code(&context.grant, code).await {
    Ok(tokens) => context.app.codex_login.persist_if_active(&context.app, &context.attempt_id, &context.provider, &tokens).await,
    Err(error) => Err(error),
  };
  match result {
    Ok(()) => {
      context.app.codex_login.finish(&context.attempt_id, "complete", None).await;
      context.grant.end.cancel();
      Html("<meta charset=\"utf-8\"><h1>ChatGPT 登录成功</h1><p>可以返回 Wish。</p><script>window.close()</script>").into_response()
    }
    Err(error) => {
      context.app.codex_login.finish(&context.attempt_id, "failed", Some(error.to_string())).await;
      context.grant.end.cancel();
      (StatusCode::BAD_GATEWAY, Html("ChatGPT login failed; return to Wish for details"))
        .into_response()
    }
  }
}

async fn exchange_code(grant: &LoginGrant, code: &str) -> Result<Tokens, ApiError> {
  let transport = ReqwestTransport::new(Default::default(), grant.proxy.clone())?;
  let mut form = reqwest::Url::parse("https://form.invalid/").map_err(ApiError::internal)?;
  form
    .query_pairs_mut()
    .append_pair("grant_type", "authorization_code")
    .append_pair("client_id", oauth::CLIENT_ID)
    .append_pair("code", code)
    .append_pair("redirect_uri", &grant.redirect_uri)
    .append_pair("code_verifier", &grant.verifier);
  let target = Outbound::new(&grant.issuer, "/oauth/token", AuthProtocol::None)?;
  let call = target.dispatch(
    Draft {
      method: Method::Post,
      path: None,
      query: Vec::new(),
      headers: vec![("content-type".to_owned(), "application/x-www-form-urlencoded".to_owned())],
      body: form.query().unwrap_or_default().as_bytes().to_vec(),
    },
    &Credentials::default(),
    0,
  )?;
  let reply = transport.execute(&call).await?;
  if !reply.is_success() {
    return Err(ApiError::bad_request(format!(
      "ChatGPT token exchange returned HTTP {}",
      reply.status
    )));
  }
  let body: Value = serde_json::from_slice(&reply.body).map_err(ApiError::internal)?;
  let tokens = oauth::decode_tokens(&body)?;
  if tokens.account_id.as_deref().unwrap_or_default().is_empty() {
    return Err(ApiError::bad_request("ChatGPT login did not return a Codex account ID"));
  }
  Ok(tokens)
}

pub fn start_refresh_worker(app: &Arc<App>) {
  let worker = Arc::clone(app);
  app.tasks.spawn(async move {
    loop {
      refresh_due(&worker).await;
      tokio::select! {
        _ = worker.stop.cancelled() => break,
        _ = tokio::time::sleep(Duration::from_secs(60)) => {},
      }
    }
  });
}

async fn refresh_due(app: &Arc<App>) {
  let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
  let due: Vec<_> = app
    .providers
    .read()
    .unwrap()
    .iter()
    .filter(|(_, provider)| {
      provider.config.preset.as_deref() == Some("openai_codex")
        && provider.config.refresh_token.as_deref().is_some_and(|token| !token.is_empty())
        && provider.config.expires_at.is_some_and(|expires| expires <= now + REFRESH_MARGIN)
    })
    .map(|(id, provider)| (id.clone(), Arc::clone(provider)))
    .collect();
  for (id, provider) in due {
    match provider.client.refresh_credentials().await {
      Ok(tokens) => {
        if let Err(error) = app.persist_codex_credentials(&id, &tokens).await {
          eprintln!("Codex token refresh for {id} could not be saved: {error}");
        }
      }
      Err(error) => eprintln!("Codex token refresh for {id} failed: {error}"),
    }
  }
}
