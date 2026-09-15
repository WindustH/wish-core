//! SiliconFlow's account balance: `{code, message, status, data: {balance, chargeBalance, ...}}`.
//!
//! Conversions:
//! - `data.balance` is the granted part, `chargeBalance` the paid one, and `totalBalance` - kept
//!   here as `available` - what the two add up to, in CNY.
//!
//! Constraints:
//! - The body is only a reading under a `data` object carrying at least one of the three amounts.
//! - A `code` other than `20000`, or `status: false`, is the service rejecting the read, with its
//!   reason in `message`.
//!
//! Trade-offs:
//! - Nothing is summed here: the service reports the total itself, and what it said is what the
//!   caller sees.
//! - A missing `totalBalance` is a warning rather than an error, because the parts are still there
//!   without their total.
//! - The body names no currency; these amounts are yuan, so `currency` is CNY.

use serde_json::Value;

use crate::Error;
use crate::protocol::account_state::{
  AccountState, AccountStateProtocol, Balance, Failure, FailureKind,
};
use crate::protocol::lexical;

/// Reads the account body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `data` is missing or carries none of the three amounts.
pub fn parse(body: &Value) -> Result<AccountState, Error> {
  let Some(data) = body.get("data").filter(|data| data.is_object()) else {
    return Err(Error::Malformed("siliconflow body missing `data` object".to_owned()));
  };
  if ["balance", "chargeBalance", "totalBalance"].iter().all(|field| data.get(field).is_none()) {
    return Err(Error::Malformed("siliconflow body carries no balance".to_owned()));
  }
  let mut warnings = Vec::new();
  let mut failure = None;
  let code = body.get("code").and_then(Value::as_i64);
  if code != Some(20000) || body.get("status").and_then(Value::as_bool) == Some(false) {
    failure = Some(Failure {
      kind: FailureKind::Unknown,
      code: code.map(|code| code.to_string()),
      message: body
        .get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .unwrap_or("the balance service rejected the read")
        .to_owned(),
    });
  }
  if data.get("totalBalance").is_none() {
    warnings
      .push("totalBalance is not reported; the parts are present without their total".to_owned());
  }
  Ok(AccountState {
    protocol: AccountStateProtocol::SiliconflowBalance,
    quotas: Vec::new(),
    balances: vec![Balance {
      currency: "CNY".to_owned(),
      available: data.get("totalBalance").and_then(lexical),
      total: None,
      cash: data.get("chargeBalance").and_then(lexical),
      granted: data.get("balance").and_then(lexical),
      topped_up: None,
      voucher: None,
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
