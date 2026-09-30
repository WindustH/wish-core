//! Kimi's two account bodies: the open-platform balance, and the code companion's plan.
//!
//! Conversions:
//! - The open-platform body arrives under `data`: `available_balance`, `cash_balance` and
//!   `voucher_balance` become one [`Balance`], the paid part as `cash` and the vouchers as
//!   `voucher`, in CNY - the body names no currency of its own.
//! - The companion body counts what a subscription spent: `usages.limit_5h`, `limit_7d`,
//!   `limit_month_total` and `limit_month_code` become four [`QuotaWindow`]s, each window's
//!   `used_ratio` read into `used_percent` and `reset_time` into `resets_at`.
//! - `boosterWallet.balance` becomes a `credits` window (`amount` as `limit`, `amountLeft` as
//!   `remaining`), and `boosterWallet.monthlyChargeLimit.priceInCents` a `currency_minor` window
//!   whose name carries the currency those minor units are in.
//!
//! Constraints:
//! - The open-platform body is only a reading when `data` is an object holding at least one of the
//!   three balances.
//! - What blocks calls is `available_balance` reaching zero, not either part on its own: a negative
//!   `cash_balance` is an account in arrears and only a warning, because the two disagree on
//!   purpose.
//! - `code` other than zero or `status: false` means the service refused the read, and the reason
//!   it gives lives in `scode`.
//! - The companion body is only a reading when `usages` is an object, and carries no error of its
//!   own - a refusal there is an HTTP status, not a body.
//!
//! Trade-offs:
//! - A plan window reports no amounts, only the share of its allowance that is gone, so `used`,
//!   `limit` and `remaining` stay empty and `unit` is `unknown`.
//! - Only the two rolling windows carry a `window`: a calendar month has no fixed length, so
//!   `limit_month_total` and `limit_month_code` are named but not measured.
//! - `monthlyChargeLimitEnabled: false` is a warning and nothing more: the limit is reported while
//!   not being enforced, and this shape has no field for "reported but off".
//! - A window the companion does not report at all is a warning, so a plan that drops one still
//!   reads as the rest of itself.

use serde_json::Value;

use crate::Error;
use crate::protocol::account_state::{
  AccountState, AccountStateProtocol, Balance, Failure, FailureKind, QuotaWindow, minutes_window,
};
use crate::protocol::json_read::{convert_ratio_to_percent, read_scalar_text};

/// Reads the open-platform balance body, which needs its `data` object to be one at all.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `data` is missing or carries none of the three balances.
pub fn parse_balance(body: &Value) -> Result<AccountState, Error> {
  let Some(data) = body.get("data").filter(|data| data.is_object()) else {
    return Err(Error::Malformed("kimi balance body missing `data` object".to_owned()));
  };
  if ["available_balance", "cash_balance", "voucher_balance"]
    .iter()
    .all(|field| data.get(field).is_none())
  {
    return Err(Error::Malformed("kimi balance body carries no balance".to_owned()));
  }
  let mut state = AccountState::new(AccountStateProtocol::KimiOpenBalance);
  let code = body.get("code").and_then(Value::as_i64).unwrap_or(0);
  if code != 0 || body.get("status").and_then(Value::as_bool) == Some(false) {
    let detail = body.get("scode").and_then(Value::as_str).unwrap_or("");
    state.failure = Some(Failure::rejected(
      Some(if detail.is_empty() { code.to_string() } else { detail.to_owned() }),
      None,
      "the balance service rejected the read",
    ));
  }
  let cash = data.get("cash_balance").and_then(read_scalar_text);
  if cash.as_deref().is_some_and(|cash| cash.starts_with('-')) {
    state.warnings.push("cash_balance is negative; the account is in arrears".to_owned());
  }
  let available = data.get("available_balance").and_then(read_scalar_text);
  if state.failure.is_none()
    && available.as_deref().is_some_and(|left| left.parse::<f64>().is_ok_and(|left| left <= 0.0))
  {
    state.failure = Some(Failure {
      kind: FailureKind::Unpaid,
      code: None,
      message: "available_balance is not above zero; the service refuses calls".to_owned(),
    });
  }
  state.balances.push(Balance {
    available,
    cash,
    voucher: data.get("voucher_balance").and_then(read_scalar_text),
    ..Balance::new("CNY")
  });
  Ok(state)
}

/// The plan windows the code companion reports: the field it words each under, the id the window is
/// known by, and its length in minutes where it has a fixed one (a calendar month does not).
const WINDOWS: &[(&str, &str, Option<i64>)] = &[
  ("limit_5h", "5h", Some(300)),
  ("limit_7d", "7d", Some(10080)),
  ("limit_month_total", "month_total", None),
  ("limit_month_code", "month_code", None),
];

/// Reads the code companion's plan body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when the body has no `usages` object, or when it reports no window
/// and no wallet at all.
pub fn parse_companion(body: &Value) -> Result<AccountState, Error> {
  let Some(usages) = body.get("usages").filter(|usages| usages.is_object()) else {
    return Err(Error::Malformed("kimi companion body missing `usages` object".to_owned()));
  };
  let mut state = AccountState::new(AccountStateProtocol::KimiCodeCompanionUsage);
  for (field, id, minutes) in WINDOWS {
    let Some(limit) = usages.get(*field) else {
      state.warnings.push(format!("`{field}` window is not reported"));
      continue;
    };
    state.quotas.push(read_plan_window(field, id, *minutes, limit, &mut state.warnings));
  }
  if let Some(wallet) = body.get("boosterWallet") {
    read_wallet(wallet, &mut state);
  }
  if state.quotas.is_empty() {
    return Err(Error::Malformed("kimi companion body carries no usage".to_owned()));
  }
  Ok(state)
}

/// One plan window: no amounts, only the share of its allowance that is spent.
fn read_plan_window(
  field: &str,
  id: &str,
  minutes: Option<i64>,
  limit: &Value,
  warnings: &mut Vec<String>,
) -> QuotaWindow {
  let ratio = limit.get("used_ratio").and_then(read_scalar_text);
  if ratio.is_none() {
    warnings.push(format!("`{field}` reports no used_ratio"));
  }
  QuotaWindow {
    name: Some(field.to_owned()),
    used_percent: ratio.as_deref().and_then(convert_ratio_to_percent),
    window: minutes.map(minutes_window),
    resets_at: limit.get("reset_time").and_then(read_scalar_text),
    reached: ratio.as_deref().and_then(|ratio| ratio.parse::<f64>().ok()).map(|ratio| ratio >= 1.0),
    ..QuotaWindow::new(id, "unknown")
  }
}

/// The booster wallet: its credits as one window, its monthly charge limit as another, and a
/// warning when that limit is reported while not enforced.
fn read_wallet(wallet: &Value, state: &mut AccountState) {
  if let Some(balance) = wallet.get("balance") {
    state.quotas.push(QuotaWindow {
      name: Some(balance.get("type").and_then(Value::as_str).unwrap_or("booster").to_owned()),
      limit: balance.get("amount").and_then(read_scalar_text),
      remaining: balance.get("amountLeft").and_then(read_scalar_text),
      ..QuotaWindow::new("booster", "credits")
    });
  }
  let charge_limit = wallet.get("monthlyChargeLimit");
  if let Some(limit) =
    charge_limit.and_then(|limit| limit.get("priceInCents")).and_then(read_scalar_text)
  {
    let currency = charge_limit.and_then(|limit| limit.get("currency")).and_then(Value::as_str);
    state.quotas.push(QuotaWindow {
      // The amounts are minor units, so the currency the service named belongs in the name.
      name: Some(match currency {
        Some(currency) => format!("monthly charge limit ({currency})"),
        None => "monthly charge limit".to_owned(),
      }),
      used: wallet
        .get("monthlyUsed")
        .and_then(|used| used.get("priceInCents"))
        .and_then(read_scalar_text),
      limit: Some(limit),
      ..QuotaWindow::new("monthly_charge", "currency_minor")
    });
  }
  if wallet.get("monthlyChargeLimitEnabled").and_then(Value::as_bool) == Some(false) {
    state
      .warnings
      .push("monthlyChargeLimitEnabled is false; the charge limit is not enforced".to_owned());
  }
}
