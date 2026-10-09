//! GitHub Copilot's allowances: `GET /copilot_internal/user`, as Copilot's editors read it.
//!
//! Conversions:
//! - Each of `quota_snapshots.chat`, `.completions` and `.premium_interactions` becomes a
//!   `requests` [`QuotaWindow`] named by its key: `entitlement` is the `limit`, `quota_remaining`
//!   the `remaining`, and `percent_remaining`, the share that is left, is read as `100 - left`.
//!   Under token-based billing the amounts arrive as strings; either spelling is kept as written.
//! - The allowances renew with the month, so each window is a calendar month long, and resets at
//!   its own `quota_reset_at` where it names one, else at the body's `quota_reset_date_utc` or
//!   `quota_reset_date`.
//! - `copilot_plan` is the plan, kept as `plan_type`.
//!
//! Constraints:
//! - The body is only a reading when `quota_snapshots` is an object; a plan with no snapshot at
//!   all still names its plan.
//! - An entitlement of `0` is no allowance, as the editors read it: such a snapshot is not a
//!   window.
//!
//! Trade-offs:
//! - `has_quota` is not read as `reached`: GitHub says `false` for an allowance used up and, under
//!   token-based billing, for every one, so it says nothing on its own. The one exception is an
//!   organization's pooled premium allowance, which reads `unlimited` and pauses the seat once the
//!   pool is spent; that window is reported reached rather than unlimited.
//! - `overage_permitted` and `access_type_sku` are dropped: what happens past an allowance, and the
//!   SKU behind the plan, are not amounts.

use serde_json::{Value, json};

use crate::Error;
use crate::protocol::account_state::{AccountState, AccountStateProtocol, QuotaWindow};
use crate::protocol::json_read::read_scalar_text;

/// The snapshots read, in the order the editors show them.
const SNAPSHOTS: [&str; 3] = ["chat", "completions", "premium_interactions"];

/// Reads the account body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `quota_snapshots` is present but not an object.
pub fn parse(body: &Value) -> Result<AccountState, Error> {
  let mut state = AccountState::new(AccountStateProtocol::GitHubCopilotUsage);
  state.plan_type = body.get("copilot_plan").and_then(Value::as_str).map(str::to_owned);
  let Some(snapshots) = body.get("quota_snapshots") else { return Ok(state) };
  let snapshots = snapshots
    .as_object()
    .ok_or_else(|| Error::Malformed("copilot `quota_snapshots` is not an object".to_owned()))?;
  let renews =
    ["quota_reset_date_utc", "quota_reset_date"].iter().find_map(|key| body.get(*key)?.as_str());
  for id in SNAPSHOTS {
    let Some(snapshot) = snapshots.get(id) else { continue };
    let unlimited = snapshot.get("unlimited").and_then(Value::as_bool);
    let entitlement = snapshot.get("entitlement").and_then(read_scalar_text);
    if unlimited != Some(true)
      && entitlement.as_deref().and_then(|e| e.parse::<f64>().ok()) == Some(0.0)
    {
      continue;
    }
    // An organization's pool reads unlimited, and says it is spent with `has_quota: false`.
    let pool_spent = id == "premium_interactions"
      && unlimited == Some(true)
      && snapshot.get("has_quota").and_then(Value::as_bool) == Some(false);
    state.quotas.push(QuotaWindow {
      name: Some(id.to_owned()),
      limit: entitlement,
      remaining: snapshot.get("quota_remaining").and_then(read_scalar_text),
      used_percent: snapshot
        .get("percent_remaining")
        .and_then(read_scalar_text)
        .and_then(|left| left.parse::<f64>().ok())
        .map(|left| (100.0 - left).to_string()),
      window: Some(json!({"duration": 1, "unit": "months"})),
      // A stamp of `0` is a window that names no reset of its own.
      resets_at: snapshot
        .get("quota_reset_at")
        .and_then(read_scalar_text)
        .filter(|at| at.parse::<f64>().is_ok_and(|at| at > 0.0))
        .or_else(|| renews.map(str::to_owned)),
      reached: pool_spent.then_some(true),
      unlimited: unlimited.map(|unlimited| unlimited && !pool_spent),
      ..QuotaWindow::new(id, "requests")
    });
  }
  Ok(state)
}
