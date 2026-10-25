//! GitHub sign-in for the Copilot subscription preset: GitHub's device flow, polled here until the
//! person has typed the code, then the first Copilot session saved with the GitHub token beside it.
//! The exchanges themselves are [`copilot_oauth`]'s, and the worker that renews the session before
//! it runs out is the one Codex's tokens use.

use crate::application::app::App;
use crate::application::copilot_login::{LoginAttempt, redeem, wait_for_approval};
use crate::protocol::endpoint::copilot_oauth::{self};
use crate::server::error::ApiError;
use crate::transport::ReqwestTransport;
use axum::{
  Json,
  extract::{Path, State},
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub async fn start(
  State(app): State<Arc<App>>,
  Path(provider_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
  app.lifecycle.require_open()?;
  let account = app.get_login_account(&provider_id)?;
  if !account.config.is_copilot() {
    return Err(ApiError::bad_request("GitHub sign-in is available only for the Copilot preset"));
  }
  let proxy = app.config_file.lock().await.config.proxy.policy(account.config.proxy_enabled);
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
  let account = app.get_login_account(&provider_id)?;
  if !account.config.is_copilot() {
    return Err(ApiError::bad_request("GitHub sign-in is available only for the Copilot preset"));
  }
  let attempt = app.copilot_login.attempt.lock().await;
  let Some(attempt) = attempt.as_ref().filter(|a| a.provider == provider_id) else {
    return Ok(Json(json!({"status":"idle"})));
  };
  Ok(Json(json!({"status":attempt.status,"error":attempt.error})))
}
