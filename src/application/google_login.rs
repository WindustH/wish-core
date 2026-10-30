//! Project provisioning after a Google browser grant. Protocol functions each perform
//! one request; this layer owns the bounded onboarding wait and cancellation.
use crate::application::error::ApplicationError;
use crate::protocol::{
  attempt::Transport,
  endpoint::{Credentials, antigravity},
};
use tokio_util::sync::CancellationToken;

pub async fn discover_project<T: Transport>(
  transport: &T,
  credentials: &Credentials,
  end: &CancellationToken,
) -> Result<String, ApplicationError> {
  let loaded = antigravity::load(transport, credentials).await?;
  if let Some(project) = antigravity::project(&loaded) {
    return Ok(project);
  }
  let tier = loaded.get("allowedTiers").and_then(serde_json::Value::as_array)
    .and_then(|tiers| tiers.iter().find(|tier| tier["isDefault"] == true))
    .and_then(|tier| tier["id"].as_str())
    .ok_or_else(|| ApplicationError::invalid("Google returned no Cloud Code project or eligible default tier; complete Antigravity setup first"))?;
  for _ in 0..10 {
    let reply = tokio::select! {
      _ = end.cancelled() => return Err(ApplicationError::conflict("Google login was cancelled")),
      reply = antigravity::onboard(transport, credentials, tier) => reply?,
    };
    if reply["done"] == true {
      return reply.get("response").and_then(antigravity::project).ok_or_else(|| {
        ApplicationError::invalid("Google onboarding returned no Cloud Code project")
      });
    }
    tokio::select! {
      _ = end.cancelled() => return Err(ApplicationError::conflict("Google login was cancelled")),
      _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {},
    }
  }
  Err(ApplicationError::invalid("Google project provisioning did not finish; try signing in again"))
}
