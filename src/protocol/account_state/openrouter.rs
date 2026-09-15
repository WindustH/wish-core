//! OpenRouter's two account views: the quota of one key, and the credits of the whole account.
//!
//! Conversions:
//! - `data.usage` (what this key has spent, in dollars) and `data.limit` (its ceiling) become one
//!   `usd` [`QuotaWindow`] named `key_quota`, `reached` once the spend has met the ceiling.
//! - `data.rate_limit.requests` becomes a second `requests` window, whose `window` keeps the
//!   interval exactly as the service worded it (`10s`).
//! - `data.total_credits` and `data.total_usage` become one `usd` window named `credits`: what the
//!   account was topped up with, against what all of its keys have spent since.
//!
//! Constraints:
//! - Both bodies are only a reading under a `data` object, and one carrying no amount at all is a
//!   malformed body rather than an empty reading.
//! - `limit: null` is a ceiling the service does not enforce: it is read as `unlimited`, which is
//!   neither zero nor a missing field.
//!
//! Trade-offs:
//! - Neither view reports what is left, and nothing is subtracted here: `limit - usage` is
//!   arithmetic the caller can do, and this shape will not guess at.
//! - `data.label` names the key and is kept as the window's name; the rest of the key object is
//!   not represented beyond a free-tier warning.

use serde_json::{Value, json};

use crate::Error;
use crate::protocol::account_state::{AccountState, AccountStateProtocol, QuotaWindow};
use crate::protocol::lexical;

/// Reads the credits body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `data` is missing, or when it reports neither of the two
/// amounts.
pub fn parse_credits(body: &Value) -> Result<AccountState, Error> {
  let Some(data) = body.get("data").filter(|data| data.is_object()) else {
    return Err(Error::Malformed("openrouter credits body missing `data` object".to_owned()));
  };
  let topped_up = data.get("total_credits").and_then(lexical);
  let spent = data.get("total_usage").and_then(lexical);
  if topped_up.is_none() && spent.is_none() {
    return Err(Error::Malformed("openrouter credits body carries no amount".to_owned()));
  }
  let reached = match (spent.as_deref(), topped_up.as_deref()) {
    (Some(spent), Some(topped_up)) => spent
      .parse::<f64>()
      .ok()
      .zip(topped_up.parse::<f64>().ok())
      .map(|(spent, topped_up)| spent >= topped_up),
    _ => None,
  };
  Ok(AccountState {
    protocol: AccountStateProtocol::OpenrouterCredits,
    quotas: vec![QuotaWindow {
      id: "credits".to_owned(),
      name: None,
      unit: "usd".to_owned(),
      // What the account paid in, against what every key of it has spent.
      used: spent,
      limit: topped_up,
      remaining: None,
      used_percent: None,
      window: None,
      resets_at: None,
      reached,
      unlimited: None,
    }],
    balances: Vec::new(),
    failure: None,
    warnings: Vec::new(),
    availability: None,
    plan_type: None,
  })
}

/// Reads the key body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `data` is missing, or when it carries neither a quota nor a
/// rate limit.
pub fn parse_key(body: &Value) -> Result<AccountState, Error> {
  let Some(data) = body.get("data").filter(|data| data.is_object()) else {
    return Err(Error::Malformed("openrouter key body missing `data` object".to_owned()));
  };
  let usage = data.get("usage").and_then(lexical);
  let limit = data.get("limit").and_then(lexical);
  let rate_limit = data.get("rate_limit").filter(|limit| limit.is_object());
  if usage.is_none() && limit.is_none() && rate_limit.is_none() {
    return Err(Error::Malformed("openrouter key body carries no quota".to_owned()));
  }
  let mut warnings = Vec::new();
  let mut quotas = Vec::new();
  if usage.is_some() || limit.is_some() {
    let reached = match (usage.as_deref(), limit.as_deref()) {
      (Some(used), Some(limit)) => {
        used.parse::<f64>().ok().zip(limit.parse::<f64>().ok()).map(|(used, limit)| used >= limit)
      }
      _ => None,
    };
    quotas.push(QuotaWindow {
      id: "key_quota".to_owned(),
      name: data.get("label").and_then(Value::as_str).map(str::to_owned),
      unit: "usd".to_owned(),
      used: usage,
      limit,
      remaining: None,
      used_percent: None,
      window: None,
      resets_at: None,
      reached,
      // A ceiling the service reports as `null` is one it does not enforce.
      unlimited: data.get("limit").map(Value::is_null),
    });
  }
  if let Some(rate_limit) = rate_limit {
    quotas.push(QuotaWindow {
      id: "rate_limit".to_owned(),
      name: None,
      unit: "requests".to_owned(),
      used: None,
      limit: rate_limit.get("requests").and_then(lexical),
      remaining: None,
      used_percent: None,
      // The window comes as a duration the service words itself (`10s`), so it is kept as it came.
      window: rate_limit.get("interval").map(|interval| json!({ "interval": interval })),
      resets_at: None,
      reached: None,
      unlimited: None,
    });
  }
  if data.get("is_free_tier").and_then(Value::as_bool) == Some(true) {
    warnings.push("is_free_tier is true; the key is on the free tier".to_owned());
  }
  Ok(AccountState {
    protocol: AccountStateProtocol::OpenrouterKeyQuota,
    quotas,
    balances: Vec::new(),
    failure: None,
    warnings,
    availability: None,
    plan_type: None,
  })
}
