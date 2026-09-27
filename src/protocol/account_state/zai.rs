//! Z.ai and Zhipu's coding plan monitor: `{success, code, data: {level, limits: []}}`.
//!
//! Conversions:
//! - Each `limits` entry is one rolling window. `type` says what it counts - `CREDIT_LIMIT` in the
//!   points-based plan, `TOKENS_LIMIT` in the token-based one it replaced, `TIME_LIMIT` for the
//!   number of tool calls (web search, page reading and the like; "times", not duration) - and
//!   `unit` times `number` says how long it is, where `unit` 1 is a day, 3 an hour, 5 a month and 6
//!   a week.
//! - `currentValue` is `used`, `usage` is `limit`, `remaining` is itself, `percentage` is the share
//!   spent, `nextResetTime` is `resets_at`, and a window at 100% is `reached`.
//! - `data.level` is the plan tier, kept as `plan_type`.
//!
//! Constraints:
//! - `success: false` or a `code` other than 200 is the service rejecting the read, with its reason
//!   in `msg`.
//! - A window is only as good as its `unit`/`number` pair: an unknown pair leaves the id as
//!   the bare type with a warning, because a limit with no window is not a quota.
//!
//! - `usageDetails` - how one limit's calls divide among its tools (`search-prime`, `web-reader`,
//!   `zread`) - become the window's `parts`, keyed by `modelCode`.
//!
//! Trade-offs:
//! - A limit `type` this protocol does not know keeps `unit: "unknown"` and warns, so a new plan
//!   still shows up instead of disappearing.
//! - A capacity window states its own `remaining`, which is what this reader reports; the other
//!   windows leave it empty and let the caller subtract.
//! - Team accounts answer on a different path and need their organization and project headers,
//!   which is the fetching layer's business rather than this one's: only the body is read here.

use serde_json::{Value, json};

use crate::protocol::account_state::{
  AccountState, AccountStateProtocol, Failure, FailureKind, QuotaPart, QuotaWindow,
};
use crate::protocol::read_scalar_text;

/// Reads the monitor body.
pub fn parse(body: &Value) -> AccountState {
  let data = body.get("data");
  let mut warnings = Vec::new();
  let rejected = body.get("success").and_then(Value::as_bool) == Some(false)
    || body.get("code").and_then(Value::as_i64).is_some_and(|code| code != 200);
  let failure = rejected.then(|| Failure {
    kind: FailureKind::Unknown,
    code: body.get("code").and_then(Value::as_i64).map(|code| code.to_string()),
    message: body
      .get("msg")
      .and_then(Value::as_str)
      .filter(|message| !message.is_empty())
      .unwrap_or("the monitor service rejected the read")
      .to_owned(),
  });
  let mut quotas = Vec::new();
  for limit in
    data.and_then(|data| data.get("limits")).and_then(Value::as_array).into_iter().flatten()
  {
    quotas.push(read_limit(limit, &mut warnings));
  }
  AccountState {
    protocol: AccountStateProtocol::ZaiCodingPlanMonitor,
    quotas,
    balances: Vec::new(),
    failure,
    warnings,
    availability: None,
    plan_type: data.and_then(|data| data.get("level")).and_then(Value::as_str).map(str::to_owned),
  }
}

/// Reads one `limits` entry into the window it describes.
fn read_limit(limit: &Value, warnings: &mut Vec<String>) -> QuotaWindow {
  let kind = limit.get("type").and_then(Value::as_str).unwrap_or("unknown");
  let window = limit
    .get("unit")
    .and_then(Value::as_i64)
    .zip(limit.get("number").and_then(Value::as_i64))
    .and_then(|(unit, number)| decode_window(unit, number));
  let id = match &window {
    Some((_, label)) => format!("{kind}:{label}"),
    None => {
      warnings.push(format!("`{kind}` entry has no window this protocol can read"));
      kind.to_owned()
    }
  };
  if !matches!(get_counted_unit(kind), "credits" | "tokens" | "requests") {
    warnings.push(format!("`{kind}` is a limit type this protocol does not know"));
  }
  let parts = limit
    .get("usageDetails")
    .and_then(Value::as_array)
    .into_iter()
    .flatten()
    .filter_map(|detail| {
      let id = detail.get("modelCode").and_then(Value::as_str)?;
      Some(QuotaPart { id: id.to_owned(), used: detail.get("usage").and_then(read_scalar_text) })
    })
    .collect();
  QuotaWindow {
    id,
    name: Some(kind.to_owned()),
    unit: get_counted_unit(kind).to_owned(),
    used: limit.get("currentValue").and_then(read_scalar_text),
    limit: limit.get("usage").and_then(read_scalar_text),
    remaining: limit.get("remaining").and_then(read_scalar_text),
    used_percent: limit.get("percentage").and_then(read_scalar_text),
    window: window.map(|(window, _)| window),
    resets_at: limit.get("nextResetTime").and_then(read_scalar_text),
    reached: limit.get("percentage").and_then(Value::as_f64).map(|spent| spent >= 100.0),
    unlimited: None,
    parts,
  }
}

/// What an entry's amounts count.
fn get_counted_unit(kind: &str) -> &'static str {
  match kind {
    "CREDIT_LIMIT" => "credits",
    "TOKENS_LIMIT" => "tokens",
    "TIME_LIMIT" => "requests",
    _ => "unknown",
  }
}

/// The window a `unit` and `number` pair describes, and the short name the pair reads as. `unit` 1
/// is a day, 3 an hour, 5 a month, 6 a week, and `number` multiplies it. Fixed lengths are given in
/// minutes; a calendar month has none, so it keeps its own unit.
fn decode_window(unit: i64, number: i64) -> Option<(Value, String)> {
  let (per_unit, letter) = match unit {
    1 => (1440, "d"),
    3 => (60, "h"),
    6 => (10080, "w"),
    5 => return Some((json!({ "duration": number, "unit": "months" }), format!("{number}mo"))),
    _ => return None,
  };
  Some((json!({ "duration": per_unit * number, "unit": "minutes" }), format!("{number}{letter}")))
}
