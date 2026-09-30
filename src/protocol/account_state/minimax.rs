//! `MiniMax`'s token plan and account balance.
//!
//! Conversions:
//! - Each `model_remains` entry is one model or capability carrying two rolling windows under
//!   different prefixes, `current_interval_*` (five hours) and `current_weekly_*` (a week), each
//!   becoming a [`QuotaWindow`] with `usage_count` as `used` and `total_count` as `limit`.
//! - `current_*_remaining_percent` is the share that is left, and this shape stores what is spent,
//!   so it is read as `100 - left`.
//! - `current_*_status` 1 is a window with room and 2 one that is spent; both become `reached`.
//! - The balance body's `available_amount`, `cash_balance`, `voucher_balance` and `credit_balance`
//!   become one [`Balance`], with `owed_amount` beside them as `owed`.
//!
//! Constraints:
//! - The plan body is only a reading when `model_remains` is an array.
//! - The balance body reports its own errors under `base_resp.status_code` while the HTTP status
//!   stays 200, and an `owed_amount` above zero is the service saying it will refuse calls with
//!   `1008`/`402` - the balance read itself still answers.
//!
//! Trade-offs:
//! - `remaining` stays empty on a plan window even though the wire reports a share of what is left:
//!   the share is of an allowance the wire never states in amounts, so it cannot be turned into
//!   one.
//! - `weekly_boost_permille` is a warning, not a multiplication: the boost is reported and not
//!   applied to the weekly window's counts.
//! - A missing `currency` is assumed to be CNY with a warning, because the body leaves it out while
//!   its amounts are yuan.
//! - A model entry is named by `model_name` where it has one and by its position where it does not,
//!   so two unnamed entries stay tellable apart in the ids.

use serde_json::Value;

use crate::Error;
use crate::protocol::account_state::{
  AccountState, AccountStateProtocol, Balance, Failure, FailureKind, QuotaWindow, minutes_window,
};
use crate::protocol::json_read::read_scalar_text;

/// Length of the interval window every plan entry carries.
const INTERVAL_MINUTES: i64 = 300;
/// Length of the weekly window every plan entry carries.
const WEEKLY_MINUTES: i64 = 10080;

/// Reads the token plan body, which needs `model_remains` to be one at all.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when the `model_remains` array is missing.
pub fn parse_quota(body: &Value) -> Result<AccountState, Error> {
  let remains = body.get("model_remains").and_then(Value::as_array).ok_or_else(|| {
    Error::Malformed("minimax quota body missing `model_remains` array".to_owned())
  })?;
  let mut state = AccountState::new(AccountStateProtocol::MinimaxTokenPlanRemains);
  for (index, model) in remains.iter().enumerate() {
    let name = model.get("model_name").and_then(Value::as_str).map(str::to_owned);
    let subject = name.clone().unwrap_or_else(|| format!("model_{index}"));
    state.quotas.push(parse_window(
      model,
      &subject,
      "interval",
      INTERVAL_MINUTES,
      "current_interval",
      "end_time",
      &mut state.warnings,
    ));
    state.quotas.push(parse_window(
      model,
      &subject,
      "weekly",
      WEEKLY_MINUTES,
      "current_weekly",
      "weekly_end_time",
      &mut state.warnings,
    ));
    if let Some(boost) = model.get("weekly_boost_permille").and_then(Value::as_i64)
      && boost != 0
    {
      state
        .warnings
        .push(format!("{subject} carries a weekly boost of {boost}/1000, which is not applied"));
    }
  }
  Ok(state)
}

/// Reads one rolling window of a plan entry; the interval and the weekly window carry the same
/// fields under a different prefix, and end under different names.
fn parse_window(
  model: &Value,
  subject: &str,
  label: &str,
  minutes: i64,
  prefix: &str,
  end: &str,
  warnings: &mut Vec<String>,
) -> QuotaWindow {
  let status = model.get(format!("{prefix}_status")).and_then(Value::as_i64);
  let reached = match status {
    Some(1) => Some(false),
    Some(2) => Some(true),
    Some(other) => {
      warnings.push(format!("{subject} {label} window has unknown status {other}"));
      None
    }
    None => None,
  };
  QuotaWindow {
    name: Some(subject.to_owned()),
    used: model.get(format!("{prefix}_usage_count")).and_then(read_scalar_text),
    limit: model.get(format!("{prefix}_total_count")).and_then(read_scalar_text),
    // The share spent, which this service reports as the share that is left.
    used_percent: model
      .get(format!("{prefix}_remaining_percent"))
      .and_then(Value::as_f64)
      .map(|left| (100.0 - left).to_string()),
    window: Some(minutes_window(minutes)),
    resets_at: model.get(end).and_then(read_scalar_text),
    reached,
    ..QuotaWindow::new(format!("{subject}:{label}"), "credits")
  }
}

/// Reads the balance body, whose `base_resp` reports an error while the HTTP status stays 200.
///
/// Two things here are the service saying it will refuse calls: a non-zero `base_resp` status, and
/// an `owed_amount` above zero, which the service answers with `1008`/`402` on the model wire.
pub fn parse_balance(body: &Value) -> AccountState {
  let mut state = AccountState::new(AccountStateProtocol::MinimaxAccountBalance);
  let base = body.get("base_resp");
  let status = base.and_then(|resp| resp.get("status_code")).and_then(Value::as_i64);
  state.failure = status.filter(|status| *status != 0).map(|status| {
    Failure::rejected(
      Some(status.to_string()),
      base.and_then(|resp| resp.get("status_msg")).and_then(Value::as_str),
      "the balance service rejected the read",
    )
  });
  let owed = body.get("owed_amount").and_then(read_scalar_text);
  if state.failure.is_none()
    && owed.as_deref().is_some_and(|owed| owed.parse::<f64>().is_ok_and(|owed| owed > 0.0))
  {
    state.failure = Some(Failure {
      kind: FailureKind::Unpaid,
      code: None,
      message: "owed_amount is above zero; the service refuses calls with 1008/402".to_owned(),
    });
  }
  let currency = match body.get("currency").and_then(Value::as_str) {
    Some(currency) => currency.to_owned(),
    None => {
      state.warnings.push("currency missing; assumed CNY".to_owned());
      "CNY".to_owned()
    }
  };
  state.balances.push(Balance {
    available: body.get("available_amount").and_then(read_scalar_text),
    cash: body.get("cash_balance").and_then(read_scalar_text),
    voucher: body.get("voucher_balance").and_then(read_scalar_text),
    credit: body.get("credit_balance").and_then(read_scalar_text),
    owed,
    ..Balance::new(currency)
  });
  state
}
