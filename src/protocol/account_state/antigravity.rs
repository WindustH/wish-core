//! Antigravity reports shared weekly and rolling windows, not token balances.
use super::{AccountState, AccountStateProtocol, QuotaWindow};
use crate::protocol::error::Error;
use serde_json::Value;

pub fn parse(body: &Value) -> Result<AccountState, Error> {
  let groups = body
    .get("groups")
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed("Antigravity quota has no groups array".into()))?;
  let mut state = AccountState::new(AccountStateProtocol::GoogleAntigravityQuota);
  for (group_index, group) in groups.iter().enumerate() {
    let buckets = group
      .get("buckets")
      .and_then(Value::as_array)
      .ok_or_else(|| Error::Malformed("Antigravity quota group has no buckets array".into()))?;
    for (index, bucket) in buckets.iter().enumerate() {
      let id = bucket["bucketId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("group_{group_index}_bucket_{index}"));
      let mut quota = QuotaWindow::new(id, "unknown");
      let names: Vec<_> = [group["displayName"].as_str(), bucket["displayName"].as_str()]
        .into_iter()
        .flatten()
        .collect();
      quota.name = (!names.is_empty()).then(|| names.join(" · "));
      quota.window = bucket.get("window").cloned();
      quota.resets_at = bucket["resetTime"].as_str().map(str::to_owned);
      if let Some(left) = bucket["remainingFraction"].as_f64().filter(|n| (0.0..=1.0).contains(n)) {
        quota.used_percent = Some(((1.0 - left) * 100.0).to_string());
        quota.reached = Some(left == 0.0);
      } else {
        state.warnings.push(format!("{}: remaining fraction unavailable", quota.id));
      }
      state.quotas.push(quota);
    }
  }
  Ok(state)
}
