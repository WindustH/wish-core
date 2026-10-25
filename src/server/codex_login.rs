//! Browser authorization for the Codex subscription preset: the login attempt, the callback the
//! browser returns to (or the redirect a person pastes back), and the worker that renews every
//! subscription's tokens - Codex's and Copilot's - before they run out. The token exchanges
//! themselves are [`codex_oauth`]'s and
//! [`copilot_oauth`](crate::protocol::endpoint::copilot_oauth)'s.

use crate::application::app::App;
use crate::application::codex_login::{LoginAttempt, LoginGrant};
use crate::protocol::endpoint::codex_oauth;
use crate::server::error::ApiError;
use axum::{
  Json, Router,
  extract::{Path, Query, State},
  http::StatusCode,
  response::{Html, IntoResponse, Response},
  routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
  collections::HashMap,
  sync::Arc,
  time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const LOGIN_LIFETIME: Duration = Duration::from_secs(600);

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
  app.lifecycle.require_open()?;
  let account = app.get_login_account(&provider_id)?;
  if !account.config.is_codex() {
    return Err(ApiError::bad_request("ChatGPT login is available only for the Codex preset"));
  }
  let proxy = app.config_file.lock().await.config.proxy.policy(account.config.proxy_enabled);
  let mut current = app.codex_login.attempt.lock().await;
  if let Some(previous) = current.take() {
    previous.grant.end.cancel();
  }
  let listener = bind_callback().await?;
  let port = listener.local_addr().map_err(ApiError::internal)?.port();
  let redirect_uri = format!("http://localhost:{port}/auth/callback");
  let state = uuid::Uuid::new_v4().to_string();
  let verifier = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
  let issuer = codex_oauth::issuer();
  let url = codex_oauth::authorization_url(&issuer, &redirect_uri, &state, &verifier)?;
  let attempt_id = uuid::Uuid::new_v4().to_string();
  let end = CancellationToken::new();
  let grant = LoginGrant { state, verifier, redirect_uri, issuer, proxy, end: end.clone() };
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
  let shutdown = app.lifecycle.stop.clone();
  let worker_app = Arc::clone(&app);
  app.lifecycle.tasks.spawn(async move {
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
  let account = app.get_login_account(&provider_id)?;
  if !account.config.is_codex() {
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
  app.lifecycle.require_open()?;
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
    let (seen, name) = match name.as_ref() {
      "state" => (&mut state, "state"),
      "code" => (&mut code, "code"),
      _ => continue,
    };
    if seen.replace(value.into_owned()).is_some() {
      return Err(ApiError::bad_request(format!("redirect URL has duplicate {name}")));
    }
  }
  if state.as_deref() != Some(grant.state.as_str()) {
    return Err(ApiError::bad_request("authorization state did not match"));
  }
  let code = code
    .filter(|value| !value.is_empty())
    .ok_or_else(|| ApiError::bad_request("redirect URL contains no authorization code"))?;
  if !app.codex_login.claim(&id).await {
    return Err(ApiError::conflict("login attempt is no longer pending"));
  }
  app.codex_login.redeem(&app, &id, &grant, &provider_id, &code).await?;
  Ok(Json(json!({"status":"complete"})))
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
  let redeemed = context
    .app
    .codex_login
    .redeem(&context.app, &context.attempt_id, &context.grant, &context.provider, code)
    .await;
  match redeemed {
    Ok(()) => Html("<meta charset=\"utf-8\"><h1>ChatGPT 登录成功</h1><p>可以返回 Wish。</p><script>window.close()</script>").into_response(),
    Err(_) => (StatusCode::BAD_GATEWAY, Html("ChatGPT login failed; return to Wish for details"))
      .into_response(),
  }
}
