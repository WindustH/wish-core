//! `DeepSeek` user balance: `{is_available, balance_infos: [{currency, total_balance, ...}]}`.
//!
//! Conversions:
//! - One [`Balance`] per `balance_infos` entry, in the currency the wire names: `total_balance` is
//!   `total`, `granted_balance` is `granted` and `topped_up_balance` is `topped_up`, each read with
//!   `lexical` so the digits arrive exactly as the wire wrote them.
//! - `is_available` is the service's own verdict on whether the balance still covers calls, so it
//!   becomes `availability` and, when false, an unpaid [`Failure`] - not only a warning.
//!
//! Constraints:
//! - The body is only a reading at all when `balance_infos` is an array.
//!
//! Trade-offs:
//! - The wire names the parts and never the sum, so `available` stays empty and adding them up is
//!   the caller's business.
//! - An entry without a `currency` is kept as `unknown` with a warning instead of being dropped: a
//!   balance nobody can name is still a balance.
//! - The whole-account view is this one read; there is no separate endpoint for the paid and the
//!   granted part.

use serde_json::Value;

use crate::Error;
use crate::protocol::account_state::{
  AccountState, AccountStateProtocol, Balance, Failure, FailureKind,
};
use crate::protocol::lexical;

/// Reads the balance body.
pub fn parse(body: &Value) -> Result<AccountState, Error> {
  let infos = body
    .get("balance_infos")
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed("deepseek body missing `balance_infos` array".to_owned()))?;
  let mut balances = Vec::new();
  let mut warnings = Vec::new();
  for info in infos {
    let currency = info.get("currency").and_then(Value::as_str).unwrap_or("unknown").to_owned();
    if currency == "unknown" {
      warnings.push("balance_infos entry without currency; kept as `unknown`".to_owned());
    }
    balances.push(Balance {
      currency,
      available: None,
      total: info.get("total_balance").and_then(lexical),
      cash: None,
      granted: info.get("granted_balance").and_then(lexical),
      topped_up: info.get("topped_up_balance").and_then(lexical),
      voucher: None,
      credit: None,
      owed: None,
      minor_unit: None,
    });
  }
  let (availability, failure) = match body.get("is_available").and_then(Value::as_bool) {
    Some(true) => (Some("available".to_owned()), None),
    Some(false) => (
      Some("unavailable".to_owned()),
      Some(Failure {
        kind: FailureKind::Unpaid,
        code: None,
        message: "is_available is false; the balance does not cover calls".to_owned(),
      }),
    ),
    None => {
      warnings.push("is_available missing; availability unknown".to_owned());
      (None, None)
    }
  };
  Ok(AccountState {
    protocol: AccountStateProtocol::DeepseekUserBalance,
    quotas: Vec::new(),
    balances,
    failure,
    warnings,
    availability,
    plan_type: None,
  })
}
