//! A Magpie gateway's allowances: `GET /v1/magpie/quotas`, what is left of every subscription,
//! plan and key the gateway holds, read in one go.
//!
//! Conversions:
//! - `data` lists one card per account, plan or key: `provider`, its `name`, the `plan` and the
//!   `user` it is signed in as, and its `windows`. Each window becomes one [`QuotaWindow`] whose
//!   name joins the card's name, plan and user with the window's own (`Codex · plus ·
//!   me@example.com · 5h`), so the windows of many accounts stay tellable apart in one reading.
//! - A window's `used` is already the share spent, in percent, and is kept as written; `resetsAt`
//!   is a date and kept as it came; `unlimited` is kept.
//! - A card's `error` - an account the gateway could not read - becomes a warning naming the card.
//!
//! Constraints:
//! - The body is only a reading when `data` is an array.
//! - Window ids join the card's provider, user and window name, numbered when two would collide,
//!   so a reading never claims two windows under one id.
//!
//! Trade-offs:
//! - A key's `balance` is not read: the gateway hands it over formatted for a person (`¥12.34`),
//!   not as an amount, and each vendor's own preset reads it as one.
//! - A window's `amount`, `limit` and `unit` are dropped, and every window keeps `unit:
//!   "unknown"`: the percentages are what every card reports, and the counts behind them are each
//!   vendor's own.
//! - `lastServedAt`, `last`, `kind` and the remaining card fields say where the gateway routes or
//!   how it shows a card, not what an account has left.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::Error;
use crate::protocol::account_state::{AccountState, AccountStateProtocol, QuotaWindow};
use crate::protocol::json_read::{read_scalar_text, read_trimmed_text};

/// Reads the quotas body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `data` is missing or is not an array.
pub fn parse(body: &Value) -> Result<AccountState, Error> {
  let cards = body
    .get("data")
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed("magpie quotas body missing `data` array".to_owned()))?;
  let mut state = AccountState::new(AccountStateProtocol::MagpieQuotas);
  let mut ids = BTreeSet::new();
  for card in cards {
    let read = |key| card.get(key).and_then(read_trimmed_text);
    let (provider, user) = (read("provider").unwrap_or_default(), read("user").unwrap_or_default());
    let title: Vec<String> =
      [read("name").or_else(|| read("provider")), read("plan"), read("user")]
        .into_iter()
        .flatten()
        .collect();
    if let Some(error) = read("error") {
      state.warnings.push(format!("{}: {error}", title.join(" · ")));
    }
    for window in card.get("windows").and_then(Value::as_array).into_iter().flatten() {
      let label = window.get("name").and_then(read_trimmed_text).unwrap_or_default();
      let base = format!("{provider}:{user}:{label}");
      let mut id = base.clone();
      let mut count = 1;
      while !ids.insert(id.clone()) {
        count += 1;
        id = format!("{base}:{count}");
      }
      let name =
        title.iter().cloned().chain((!label.is_empty()).then_some(label)).collect::<Vec<_>>();
      state.quotas.push(QuotaWindow {
        name: Some(name.join(" · ")),
        used_percent: window.get("used").and_then(read_scalar_text),
        resets_at: window.get("resetsAt").and_then(read_trimmed_text),
        unlimited: window.get("unlimited").and_then(Value::as_bool),
        ..QuotaWindow::new(id, "unknown")
      });
    }
  }
  Ok(state)
}
