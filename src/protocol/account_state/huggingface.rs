//! Hugging Face's account plan, read from `GET /api/whoami-v2`.
//!
//! Conversions:
//! - `billing.plan` becomes `plan_type` - the only field of this body the protocol reads.
//!
//! Constraints:
//! - The body is only a reading when it is an object: an account without billing is a well-formed
//!   answer without a plan, not an error.
//!
//! Trade-offs:
//! - The identity the same body carries - name, email, organizations - is left where it is: this
//!   shape answers what an account has left, not who it is.
//! - The service exposes no amounts here, so a caller that wants a number has to read the
//!   rate-limit headers off its own model calls instead.
//! - A missing `billing` and a `billing` without a plan are both warnings, because the difference
//!   between "not answered" and "answered as nothing" matters while a provider is being diagnosed.

use serde_json::Value;

use crate::Error;
use crate::protocol::account_state::{AccountState, AccountStateProtocol};

/// Reads the whoami body's billing part.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when the body is not an object, which is the only way a whoami
/// answer can be unreadable: an account without billing is a well-formed answer without a plan.
pub fn parse(body: &Value) -> Result<AccountState, Error> {
  if !body.is_object() {
    return Err(Error::Malformed("huggingface whoami body is not an object".to_owned()));
  }
  let mut warnings = Vec::new();
  let plan = match body.get("billing") {
    Some(billing) => billing.get("plan").and_then(Value::as_str).map(str::to_owned),
    None => {
      warnings.push("no `billing` object is reported".to_owned());
      None
    }
  };
  if plan.is_none() && body.get("billing").is_some() {
    warnings.push("`billing` carries no plan".to_owned());
  }
  Ok(AccountState {
    protocol: AccountStateProtocol::HuggingfaceWhoamiBilling,
    quotas: Vec::new(),
    balances: Vec::new(),
    failure: None,
    warnings,
    availability: None,
    plan_type: plan,
  })
}
