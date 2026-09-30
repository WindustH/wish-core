//! Codex quota: `rate_limits` with a `primary` and a `secondary` window, read from the account
//! endpoint, from the `x-codex-*` headers of a reply, or from the payload of the rate-limit frame
//! that rides a Responses stream.
//!
//! Conversions:
//! - `rate_limits.primary` and `.secondary` are two named windows, each becoming a `requests`
//!   [`QuotaWindow`] with `used`, `limit`, `remaining`, `used_percent`, `window_minutes` and
//!   `reset_at`; none of them is required, because a plan may report only one window.
//! - The account endpoint names the same two windows `rate_limit.primary_window` and
//!   `.secondary_window`, gives the window length in seconds (kept as minutes, like the rest of the
//!   module) and reports the meter's standing as `reached`. Each entry of `additional_rate_limits`
//!   is a meter of its own, and its windows are named `<limit_name>:<window>` so that two meters
//!   cannot claim one id.
//! - `plan_type` is the plan, kept as `plan_type`; a frame carries it beside `rate_limits`, and the
//!   account endpoint carries it at the top. A body that names a `status` reports it as the
//!   availability.
//! - `rate_limits.credits.balance` becomes a `credits` [`Balance`], and an `unlimited` window is
//!   added beside it when the service says the credits never run out.
//! - The `x-codex-*` headers say the same two windows in percentages: a used percent, a window
//!   length in minutes and a reset as a unix second stamp per window.
//!
//! Constraints:
//! - The payload is only a reading when `rate_limits` is present at all; a head only carries one
//!   when it names at least one window.
//! - Credits that do not exist are not zero credits: a balance is only reported under
//!   `credits.exists` or `credits.has_credits`, because "no credits" and "no credit line" are
//!   different facts.
//! - Nothing here names what the metered amounts count, so the plan windows keep `unit: "unknown"`
//!   rather than claim a unit the service never named; only the credit line has one.
//!
//! Trade-offs:
//! - An unlimited balance is reported as an unlimited window rather than as a number: an unlimited
//!   balance read as a spendable amount is a lie with a decimal point.
//! - `reached` is only reported where the wire says it, which is `limit_reached` on the account
//!   endpoint: a head or a frame says how full a window is, not whether the service will refuse the
//!   next call.
//! - The window ids are the wire's own names for them, so nothing here claims to know what a
//!   "primary" window measures.
//! - `x-codex-limit-name`, `x-codex-promo-message` and `x-codex-rate-limit-reached-type` are
//!   dropped: a meter's name, a message for a person to read and a reached kind are not amounts,
//!   and the percentages already say what they say.
//! - A head that says credits exist without saying how many is reported as a balance with no
//!   amount and a warning, because dropping it would deny a credit line the service did report.
//! - The identity a body echoes (`user_id`, `email`, `account_id`), each meter's `metered_feature`
//!   and `reset_after_seconds` are dropped: the snapshot is about what an account has left, not who
//!   it is, and the remaining seconds say what `reset_at` already says.
//! - `code_review_rate_limit` is not read: it is null on every body seen, and a meter nobody has
//!   seen is a shape to guess at.

use serde_json::Value;

use crate::Error;
use crate::protocol::account_state::{
  AccountState, AccountStateProtocol, Balance, QuotaWindow, minutes_window, read_header,
};
use crate::protocol::json_read::read_scalar_text;

/// Reads the quota payload of a rate-limit frame, which needs `rate_limits` to be one at all.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `rate_limits` is missing.
pub fn parse_rate_limit_payload(body: &Value) -> Result<AccountState, Error> {
  let limits = body
    .get("rate_limits")
    .ok_or_else(|| Error::Malformed("codex quota body missing `rate_limits`".to_owned()))?;
  let mut state = AccountState::new(AccountStateProtocol::OpenAiCodexQuotaHeaders);
  for id in ["primary", "secondary"] {
    let Some(window) = limits.get(id) else { continue };
    state.quotas.push(QuotaWindow {
      window: window.get("window_minutes").cloned().map(minutes_window),
      ..read_window_amounts(id, window)
    });
  }
  if let Some(credits) = limits.get("credits") {
    let exists =
      credits.get("exists").or_else(|| credits.get("has_credits")).and_then(Value::as_bool)
        == Some(true);
    let unlimited = credits.get("unlimited").and_then(Value::as_bool) == Some(true);
    if exists {
      state.balances.push(Balance {
        available: credits.get("balance").and_then(read_scalar_text),
        ..Balance::new("credits")
      });
      if unlimited {
        state.quotas.push(unlimited_credits());
      }
    }
  }
  state.plan_type = body
    .get("plan_type")
    .or_else(|| limits.get("plan_type"))
    .and_then(Value::as_str)
    .map(str::to_owned);
  Ok(state)
}

/// Reads the account endpoint's reading (`GET /backend-api/wham/usage`), the active truth about a
/// subscription's quota.
///
/// A missing `rate_limit` is not an error: the body may still name the plan and the account's
/// standing, which is a reading of its own.
pub fn parse_usage(body: &Value) -> AccountState {
  let mut state = AccountState::new(AccountStateProtocol::OpenAiCodexUsage);
  if let Some(limit) = body.get("rate_limit").filter(|limit| limit.is_object()) {
    append_meter_windows("", limit, &mut state.quotas);
  }
  for meter in body.get("additional_rate_limits").and_then(Value::as_array).into_iter().flatten() {
    let Some(name) = meter.get("limit_name").and_then(Value::as_str) else {
      state.warnings.push("an additional rate limit without a `limit_name` was dropped".to_owned());
      continue;
    };
    match meter.get("rate_limit").filter(|limit| limit.is_object()) {
      Some(limit) => append_meter_windows(name, limit, &mut state.quotas),
      None => state.warnings.push(format!("the `{name}` meter reports no rate limit")),
    }
  }
  state.availability = body.get("status").and_then(Value::as_str).map(str::to_owned);
  state.plan_type = body.get("plan_type").and_then(Value::as_str).map(str::to_owned);
  state
}

/// One meter's two windows and the meter's own standing: the account's meter when `name` is empty,
/// an additional meter (`gpt-reserve`) otherwise.
fn append_meter_windows(name: &str, limit: &Value, quotas: &mut Vec<QuotaWindow>) {
  let reached = limit.get("limit_reached").and_then(Value::as_bool);
  for window in ["primary", "secondary"] {
    let key = format!("{window}_window");
    let Some(value) = limit.get(&key).filter(|value| value.is_object()) else { continue };
    let id = if name.is_empty() { window.to_owned() } else { format!("{name}:{window}") };
    quotas.push(QuotaWindow {
      name: (!name.is_empty()).then(|| name.to_owned()),
      window: value
        .get("limit_window_seconds")
        .and_then(Value::as_u64)
        .map(|seconds| minutes_window(seconds / 60)),
      reached,
      ..read_window_amounts(id, value)
    });
  }
}

/// The amounts one window reports the same way in a frame and on the account endpoint: what is
/// spent, the allowance, what is left, the share spent and when it resets.
fn read_window_amounts(id: impl Into<String>, window: &Value) -> QuotaWindow {
  QuotaWindow {
    used: window.get("used").and_then(read_scalar_text),
    limit: window.get("limit").and_then(read_scalar_text),
    remaining: window.get("remaining").and_then(read_scalar_text),
    used_percent: window.get("used_percent").and_then(read_scalar_text),
    resets_at: window
      .get("reset_at")
      .or_else(|| window.get("resets_at"))
      .and_then(read_scalar_text),
    ..QuotaWindow::new(id, "unknown")
  }
}

/// The window that says a subscription's credits never run out.
fn unlimited_credits() -> QuotaWindow {
  QuotaWindow { unlimited: Some(true), ..QuotaWindow::new("credits", "credits") }
}

/// Reads the `x-codex-*` headers of a reply, when it carried any.
///
/// `None` means the head said nothing about the account, which is not the same as a head that said
/// the account has nothing left.
pub fn parse_headers(headers: &[(String, String)]) -> Option<AccountState> {
  let mut state = AccountState::new(AccountStateProtocol::OpenAiCodexQuotaHeaders);
  for id in ["primary", "secondary"] {
    let percent = read_header(headers, &format!("x-codex-{id}-used-percent"));
    let window = read_header(headers, &format!("x-codex-{id}-window-minutes"));
    let resets_at = read_header(headers, &format!("x-codex-{id}-reset-at"));
    if percent.is_none() && window.is_none() && resets_at.is_none() {
      continue;
    }
    state.quotas.push(QuotaWindow {
      used_percent: percent,
      // A head spells the length as text; a number is what the endpoint's own reading carries.
      window: window.and_then(|minutes| minutes.parse::<u64>().ok()).map(minutes_window),
      resets_at,
      ..QuotaWindow::new(id, "unknown")
    });
  }
  if read_header(headers, "x-codex-credits-unlimited").as_deref() == Some("true") {
    state.quotas.push(unlimited_credits());
  } else if read_header(headers, "x-codex-credits-has-credits").as_deref() == Some("true") {
    state.balances.push(Balance::new("credits"));
    state.warnings.push("the head reports credits without a balance".to_owned());
  }
  if state.quotas.is_empty() && state.balances.is_empty() {
    return None;
  }
  Some(state)
}
