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

use serde_json::{Value, json};

use crate::Error;
use crate::protocol::account_state::{
  AccountState, AccountStateProtocol, Balance, Failure, FailureKind, QuotaWindow,
};
use crate::protocol::{lexical, percent_from_ratio};

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
  let mut warnings = Vec::new();
  let mut failure = None;
  let code = body.get("code").and_then(Value::as_i64).unwrap_or(0);
  if code != 0 || body.get("status").and_then(Value::as_bool) == Some(false) {
    let detail = body.get("scode").and_then(Value::as_str).unwrap_or("");
    failure = Some(Failure {
      kind: FailureKind::Unknown,
      code: Some(if detail.is_empty() { code.to_string() } else { detail.to_owned() }),
      message: "the balance service rejected the read".to_owned(),
    });
  }
  let cash = data.get("cash_balance").and_then(lexical);
  if cash.as_deref().is_some_and(|cash| cash.starts_with('-')) {
    warnings.push("cash_balance is negative; the account is in arrears".to_owned());
  }
  let available = data.get("available_balance").and_then(lexical);
  if failure.is_none()
    && available.as_deref().is_some_and(|left| left.parse::<f64>().is_ok_and(|left| left <= 0.0))
  {
    failure = Some(Failure {
      kind: FailureKind::Unpaid,
      code: None,
      message: "available_balance is not above zero; the service refuses calls".to_owned(),
    });
  }
  Ok(AccountState {
    protocol: AccountStateProtocol::KimiOpenBalance,
    quotas: Vec::new(),
    balances: vec![Balance {
      currency: "CNY".to_owned(),
      available,
      total: None,
      cash,
      granted: None,
      topped_up: None,
      voucher: data.get("voucher_balance").and_then(lexical),
      credit: None,
      owed: None,
      minor_unit: None,
    }],
    failure,
    warnings,
    availability: None,
    plan_type: None,
  })
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
  let mut warnings = Vec::new();
  let mut quotas = Vec::new();
  for (field, id, minutes) in WINDOWS {
    let Some(limit) = usages.get(*field) else {
      warnings.push(format!("`{field}` window is not reported"));
      continue;
    };
    let ratio = limit.get("used_ratio").and_then(lexical);
    if ratio.is_none() {
      warnings.push(format!("`{field}` reports no used_ratio"));
    }
    quotas.push(QuotaWindow {
      id: (*id).to_owned(),
      name: Some((*field).to_owned()),
      // A plan window reports no amounts, only the share of its allowance that is spent.
      unit: "unknown".to_owned(),
      used: None,
      limit: None,
      remaining: None,
      used_percent: ratio.as_deref().and_then(percent_from_ratio),
      window: minutes.map(|minutes| json!({ "duration": minutes, "unit": "minutes" })),
      resets_at: limit.get("reset_time").and_then(lexical),
      reached: ratio
        .as_deref()
        .and_then(|ratio| ratio.parse::<f64>().ok())
        .map(|ratio| ratio >= 1.0),
      unlimited: None,
    });
  }
  let wallet = body.get("boosterWallet");
  if let Some(balance) = wallet.and_then(|wallet| wallet.get("balance")) {
    quotas.push(QuotaWindow {
      id: "booster".to_owned(),
      name: Some(balance.get("type").and_then(Value::as_str).unwrap_or("booster").to_owned()),
      unit: "credits".to_owned(),
      used: None,
      limit: balance.get("amount").and_then(lexical),
      remaining: balance.get("amountLeft").and_then(lexical),
      used_percent: None,
      window: None,
      resets_at: None,
      reached: None,
      unlimited: None,
    });
  }
  let charge_limit = wallet.and_then(|wallet| wallet.get("monthlyChargeLimit"));
  if let Some(limit) = charge_limit.and_then(|limit| limit.get("priceInCents")).and_then(lexical) {
    let currency = charge_limit.and_then(|limit| limit.get("currency")).and_then(Value::as_str);
    quotas.push(QuotaWindow {
      id: "monthly_charge".to_owned(),
      // The amounts are minor units, so the currency the service named belongs in the name.
      name: Some(match currency {
        Some(currency) => format!("monthly charge limit ({currency})"),
        None => "monthly charge limit".to_owned(),
      }),
      unit: "currency_minor".to_owned(),
      used: wallet
        .and_then(|wallet| wallet.get("monthlyUsed"))
        .and_then(|used| used.get("priceInCents"))
        .and_then(lexical),
      limit: Some(limit),
      remaining: None,
      used_percent: None,
      window: None,
      resets_at: None,
      reached: None,
      unlimited: None,
    });
  }
  if wallet.and_then(|wallet| wallet.get("monthlyChargeLimitEnabled")).and_then(Value::as_bool)
    == Some(false)
  {
    warnings
      .push("monthlyChargeLimitEnabled is false; the charge limit is not enforced".to_owned());
  }
  if quotas.is_empty() {
    return Err(Error::Malformed("kimi companion body carries no usage".to_owned()));
  }
  Ok(AccountState {
    protocol: AccountStateProtocol::KimiCodeCompanionUsage,
    quotas,
    balances: Vec::new(),
    failure: None,
    warnings,
    availability: None,
    plan_type: None,
  })
}
