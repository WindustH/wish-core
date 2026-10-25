//! Copilot login lifecycle, independent of HTTP requests.
use crate::application::{app::App, codex_login::save_credentials, error::ApplicationError};
use crate::protocol::endpoint::{
  Credentials,
  copilot_oauth::{self, DeviceCode, DevicePoll},
};
use crate::transport::ReqwestTransport;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
#[derive(Default)]
pub struct CopilotLoginManager {
  pub(crate) attempt: Mutex<Option<LoginAttempt>>,
}

pub(crate) struct LoginAttempt {
  pub(crate) id: String,
  pub(crate) provider: String,
  pub(crate) status: &'static str,
  pub(crate) error: Option<String>,
  pub(crate) end: CancellationToken,
}

/// Polls a device sign-in at the pace GitHub asks until the person approves it, GitHub refuses it
/// or the code expires: the GitHub token, or the status and reason the attempt ends with. `None`
/// when the attempt was replaced or the server is stopping.
pub(crate) async fn wait_for_approval(
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
pub(crate) async fn redeem(
  app: &App,
  transport: &ReqwestTransport,
  id: &str,
  provider: &str,
  github: String,
) -> Result<(), ApplicationError> {
  let credentials = Credentials { refresh_token: Some(github), ..Credentials::default() };
  let tokens = copilot_oauth::exchange(transport, &credentials).await.map_err(|error| {
    ApplicationError::invalid(format!("GitHub signed in, but Copilot refused the account: {error}"))
  })?;
  let attempt = app.copilot_login.attempt.lock().await;
  if !attempt.as_ref().is_some_and(|a| a.id == id && a.status == "pending") {
    return Err(ApplicationError::conflict("sign-in attempt was replaced"));
  }
  save_credentials(app, provider, &credentials.renew(&tokens)).await
}

impl CopilotLoginManager {
  pub(crate) async fn finish(&self, id: &str, status: &'static str, error: Option<String>) {
    let mut attempt = self.attempt.lock().await;
    if let Some(current) = attempt.as_mut().filter(|a| a.id == id && a.status == "pending") {
      current.status = status;
      current.error = error;
    }
  }
}
