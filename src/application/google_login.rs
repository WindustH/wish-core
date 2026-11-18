//! Antigravity enrollment after a Google browser grant. A Cloud Code project is optional
//! for consumer accounts; this layer owns the bounded onboarding wait and cancellation.
use crate::application::error::ApplicationError;
use crate::protocol::{
  attempt::Transport,
  endpoint::{Credentials, antigravity},
};
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub async fn enroll<T: Transport>(
  transport: &T,
  credentials: &Credentials,
  end: &CancellationToken,
) -> Result<Option<String>, ApplicationError> {
  tokio::select! {
    biased;
    _ = end.cancelled() => Err(ApplicationError::conflict("Google login was cancelled")),
    result = tokio::time::timeout(Duration::from_secs(60), provision(transport, credentials)) =>
      result.unwrap_or_else(|_| Err(ApplicationError::invalid("Google account setup timed out; try signing in again"))),
  }
}

async fn provision<T: Transport>(
  transport: &T,
  credentials: &Credentials,
) -> Result<Option<String>, ApplicationError> {
  let loaded = antigravity::load(transport, credentials).await?;
  let configured = credentials.project.as_deref().map(str::trim).filter(|p| !p.is_empty());
  if let Some(project) = antigravity::project(&loaded) {
    return Ok(Some(project));
  }
  // agy accepts an enrolled account without a project. Do not onboard it again.
  if loaded.get("currentTier").is_some_and(|tier| tier.is_object()) {
    return Ok(configured.map(str::to_owned));
  }
  let tiers = loaded.get("allowedTiers").and_then(Value::as_array);
  // Antigravity's consumer enrollment uses free-tier even if it is not marked default.
  let tier = tiers.and_then(|tiers| {
    tiers
      .iter()
      .find(|tier| matches!(tier["id"].as_str(), Some("free-tier" | "FREE")))
      .or_else(|| tiers.iter().find(|tier| tier["isDefault"] == true))
  });
  if tier.is_none()
    && loaded.get("ineligibleTiers").and_then(Value::as_array).is_some_and(|v| !v.is_empty())
  {
    return Err(enrollment_error(&loaded));
  }
  let tier_id =
    tier.and_then(|tier| tier["id"].as_str()).filter(|id| !id.is_empty()).unwrap_or("free-tier");
  if tier.is_some_and(|tier| tier["userDefinedCloudaicompanionProject"] == true)
    && configured.is_none()
  {
    return Err(ApplicationError::invalid(
      "Google requires your own Cloud project for this tier; enter its project ID in the account's project field and sign in again",
    ));
  }
  let mut reply = antigravity::onboard(transport, credentials, tier_id).await?;
  let name = reply.get("name").and_then(Value::as_str).map(str::to_owned);
  for attempt in 0..=20 {
    if reply["done"] == true {
      if let Some(project) = reply.get("response").and_then(antigravity::project) {
        return Ok(Some(project));
      }
      // Refresh account state after enrollment, as agy does. A successful operation
      // with no project is normal: credentials and subsequent requests remain usable.
      let reloaded = antigravity::load(transport, credentials).await?;
      if reloaded.get("currentTier").is_none_or(|tier| !tier.is_object())
        && reloaded.get("ineligibleTiers").and_then(Value::as_array).is_some_and(|v| !v.is_empty())
      {
        return Err(enrollment_error(&reloaded));
      }
      return Ok(antigravity::project(&reloaded).or_else(|| configured.map(str::to_owned)));
    }
    if attempt == 20 {
      break;
    }
    let name = name.as_deref().filter(|name| !name.is_empty()).ok_or_else(|| {
      ApplicationError::invalid("Google account setup returned neither a completed operation nor an operation name; try signing in again")
    })?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    reply = antigravity::operation(transport, credentials, name).await?;
  }
  Err(ApplicationError::invalid("Google account setup did not finish; try signing in again"))
}

fn enrollment_error(body: &Value) -> ApplicationError {
  let reasons: Vec<_> = body
    .get("ineligibleTiers")
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .filter_map(|tier| tier.get("reasonMessage").and_then(Value::as_str))
    .filter(|reason| !reason.trim().is_empty())
    .take(3)
    .map(|reason| reason.chars().take(300).collect::<String>())
    .collect();
  let detail = if reasons.is_empty() {
    "This account is not eligible for Antigravity; check its access in Antigravity".to_owned()
  } else {
    reasons.join("; ")
  };
  ApplicationError::invalid(format!(
    "Google authorization succeeded, but account setup failed. {detail}"
  ))
}
