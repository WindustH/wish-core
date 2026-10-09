//! GitHub sign-in for the Copilot subscription preset: GitHub's device flow, polled here until the
//! person has typed the code, then the first Copilot session saved with the GitHub token beside it.
//! The exchanges themselves are [`copilot_oauth`]'s, and the worker that renews the session before
//! it runs out is the one Codex's tokens use.

use crate::protocol::endpoint::Credentials;
use crate::protocol::endpoint::copilot_oauth::{self, DeviceCode, DevicePoll};
use crate::server::{app::App, codex_login::save_credentials, error::ApiError};
use crate::transport::ReqwestTransport;
use axum::{
  Json,
  extract::{Path, State},
};
use serde_json::{Value, json};
use std::{
  sync::Arc,
  time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct CopilotLoginManager {
  attempt: Mutex<Option<LoginAttempt>>,
}

struct LoginAttempt {
  id: String,
  provider: String,
  status: &'static str,
  error: Option<String>,
  end: CancellationToken,
}

pub async fn start(
  State(app): State<Arc<App>>,
  Path(provider_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  let provider = app.get_provider(&provider_id)?;
  if !provider.config.is_copilot() {
    return Err(ApiError::bad_request("GitHub sign-in is available only for the Copilot preset"));
  }
  let proxy = app.config_file.lock().await.config.proxy.policy(provider.config.proxy_enabled);
  let transport = ReqwestTransport::new(Default::default(), proxy)?;
  let code = copilot_oauth::request_device_code(&transport).await?;
  let id = uuid::Uuid::new_v4().to_string();
  let end = CancellationToken::new();
  {
    let mut current = app.copilot_login.attempt.lock().await;
    if let Some(previous) = current.take() {
      previous.end.cancel();
    }
    *current = Some(LoginAttempt {
      id: id.clone(),
      provider: provider_id.clone(),
      status: "pending",
      error: None,
      end: end.clone(),
    });
  }
  let answer = json!({"verification_uri": code.verification_uri, "user_code": code.user_code,
    "expires_in": code.expires_in});
  let worker = Arc::clone(&app);
  app.lifecycle.tasks.spawn(async move {
    let (status, error) = match wait_for_approval(&worker, &transport, &code, &end).await {
      Some(Ok(github)) => match redeem(&worker, &transport, &id, &provider_id, github).await {
        Ok(()) => ("complete", None),
        Err(error) => ("failed", Some(error.to_string())),
      },
      Some(Err((status, error))) => (status, error),
      // Replaced, or the server is stopping: nothing is left to tell.
      None => return,
    };
    worker.copilot_login.finish(&id, status, error).await;
  });
  Ok(Json(answer))
}

pub async fn status(
  State(app): State<Arc<App>>,
  Path(provider_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  let provider = app.get_provider(&provider_id)?;
  if !provider.config.is_copilot() {
    return Err(ApiError::bad_request("GitHub sign-in is available only for the Copilot preset"));
  }
  let attempt = app.copilot_login.attempt.lock().await;
  let Some(attempt) = attempt.as_ref().filter(|a| a.provider == provider_id) else {
    return Ok(Json(json!({"status":"idle"})));
  };
  Ok(Json(json!({"status":attempt.status,"error":attempt.error})))
}

/// Polls a device sign-in at the pace GitHub asks until the person approves it, GitHub refuses it
/// or the code expires: the GitHub token, or the status and reason the attempt ends with. `None`
/// when the attempt was replaced or the server is stopping.
async fn wait_for_approval(
  app: &App,
  transport: &ReqwestTransport,
  code: &DeviceCode,
  end: &CancellationToken,
) -> Option<Result<String, (&'static str, Option<String>)>> {
  let deadline = Instant::now() + Duration::from_secs(code.expires_in);
  let mut interval = Duration::from_secs(code.interval.max(1));
  loop {
    tokio::select! {
      _ = end.cancelled() => return None,
      _ = app.lifecycle.stop.cancelled() => return None,
      _ = tokio::time::sleep(interval) => {},
    }
    if Instant::now() >= deadline {
      return Some(Err(("expired", None)));
    }
    match copilot_oauth::poll_device_code(transport, &code.device_code).await {
      Ok(DevicePoll::Approved(token)) => return Some(Ok(token)),
      Ok(DevicePoll::SlowDown) => interval += Duration::from_secs(5),
      Ok(DevicePoll::Refused(reason)) => return Some(Err(("failed", Some(reason)))),
      // A poll that did not get through is asked again; the code's expiry bounds the wait.
      Ok(DevicePoll::Pending) | Err(_) => {}
    }
  }
}

/// Exchanges an approved sign-in's GitHub token for a first Copilot session, which also tells an
/// account without Copilot apart, and saves both while the attempt is still the current one.
async fn redeem(
  app: &App,
  transport: &ReqwestTransport,
  id: &str,
  provider: &str,
  github: String,
) -> Result<(), ApiError> {
  let credentials = Credentials { refresh_token: Some(github), ..Credentials::default() };
  let tokens = copilot_oauth::exchange(transport, &credentials).await.map_err(|error| {
    ApiError::bad_request(format!("GitHub signed in, but Copilot refused the account: {error}"))
  })?;
  let attempt = app.copilot_login.attempt.lock().await;
  if !attempt.as_ref().is_some_and(|a| a.id == id && a.status == "pending") {
    return Err(ApiError::conflict("sign-in attempt was replaced"));
  }
  save_credentials(app, provider, &credentials.renew(&tokens)).await
}

impl CopilotLoginManager {
  async fn finish(&self, id: &str, status: &'static str, error: Option<String>) {
    let mut attempt = self.attempt.lock().await;
    if let Some(current) = attempt.as_mut().filter(|a| a.id == id && a.status == "pending") {
      current.status = status;
      current.error = error;
    }
  }
}
