//! Qwen's workspace quota: `{code, message, data: {status, tpm_*, rpm_*, monthly_*}}`.
//!
//! Conversions:
//! - The per-minute windows `tpm_used`/`tpm_limit` and `rpm_used`/`rpm_limit` become two
//!   [`QuotaWindow`]s of a minute each, one in tokens and one in requests.
//! - `monthly_spend` against `monthly_budget` becomes a `monthly_budget` window whose `unit` is the
//!   currency `quota_currency` names, lowercased.
//! - `status` is the service's own verdict on the workspace: `normal` and `warning` are
//!   `available`, `frozen` is `unavailable` with an unpaid [`Failure`].
//!
//! Constraints:
//! - The body is only a reading under a `data` object that reports at least one window.
//! - `code` is a string on this wire, and anything but `"200"` is the service rejecting the read,
//!   with its reason in `message`.
//!
//! Trade-offs:
//! - A window the body omits is a warning rather than an empty window, so a workspace without a
//!   monthly budget still reads as the windows it does have.
//! - A budget in a currency the body does not name keeps `unit: "currency"` with a warning: an
//!   amount nobody can name is still an amount.
//! - `warning` is `available` plus a warning - the workspace still answers, and what it is close to
//!   is the caller's to watch - while an unknown `status` is neither verdict, only a warning.

use serde_json::{Value, json};

use crate::Error;
use crate::protocol::account_state::{
  AccountState, AccountStateProtocol, Failure, FailureKind, QuotaWindow,
};
use crate::protocol::lexical;

/// Reads the workspace quota body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `data` is missing or reports none of the three windows.
pub fn parse_workspace_quota(body: &Value) -> Result<AccountState, Error> {
  let Some(data) = body.get("data").filter(|data| data.is_object()) else {
    return Err(Error::Malformed("qwen workspace body missing `data` object".to_owned()));
  };
  let mut warnings = Vec::new();
  let mut failure = None;
  let code = body.get("code").and_then(lexical);
  if code.as_deref() != Some("200") {
    failure = Some(Failure {
      kind: FailureKind::Unknown,
      code,
      message: body
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .unwrap_or("the workspace service rejected the read")
        .to_owned(),
    });
  }
  let mut quotas = Vec::new();
  for (id, unit, used_field, limit_field) in
    [("tpm", "tokens", "tpm_used", "tpm_limit"), ("rpm", "requests", "rpm_used", "rpm_limit")]
  {
    let used = data.get(used_field);
    let limit = data.get(limit_field);
    if used.is_none() && limit.is_none() {
      warnings.push(format!("`{id}` window is not reported"));
      continue;
    }
    quotas.push(QuotaWindow {
      id: id.to_owned(),
      name: None,
      unit: unit.to_owned(),
      used: used.and_then(lexical),
      limit: limit.and_then(lexical),
      remaining: None,
      used_percent: None,
      window: Some(json!({ "duration": 1, "unit": "minutes" })),
      resets_at: None,
      reached: None,
      unlimited: None,
    });
  }
  if data.get("monthly_budget").is_some() || data.get("monthly_spend").is_some() {
    let currency = data.get("quota_currency").and_then(Value::as_str);
    if currency.is_none() {
      warnings
        .push("quota_currency is not reported; the budget is in no named currency".to_owned());
    }
    quotas.push(QuotaWindow {
      id: "monthly_budget".to_owned(),
      name: None,
      // The amounts are the service's own decimals in the currency it names, so the unit is that
      // currency rather than a minor unit of it.
      unit: currency.unwrap_or("currency").to_lowercase(),
      used: data.get("monthly_spend").and_then(lexical),
      limit: data.get("monthly_budget").and_then(lexical),
      remaining: None,
      used_percent: None,
      window: None,
      resets_at: None,
      reached: None,
      unlimited: None,
    });
  } else {
    warnings.push("`monthly_budget` window is not reported".to_owned());
  }
  let availability = match data.get("status").and_then(Value::as_str) {
    Some("normal") => Some("available".to_owned()),
    Some("warning") => {
      warnings.push("status is warning; the workspace is close to its budget".to_owned());
      Some("available".to_owned())
    }
    Some("frozen") => {
      failure.get_or_insert(Failure {
        kind: FailureKind::Unpaid,
        code: None,
        message: "status is frozen; the workspace is over its budget".to_owned(),
      });
      Some("unavailable".to_owned())
    }
    Some(reported) => {
      warnings.push(format!("status `{reported}` is not one this protocol knows"));
      None
    }
    None => {
      warnings.push("status is not reported".to_owned());
      None
    }
  };
  if quotas.is_empty() {
    return Err(Error::Malformed("qwen workspace body carries no quota".to_owned()));
  }
  Ok(AccountState {
    protocol: AccountStateProtocol::QwenWorkspaceQuota,
    quotas,
    balances: Vec::new(),
    failure,
    warnings,
    availability,
    plan_type: None,
  })
}
