//! Rate limits read from a reply's headers: the passive dialects, whose services report what is
//! left only on a call that is really served.
//!
//! Conversions:
//! - Each dialect names the headers its service sends; a header becomes the `remaining`, `limit` or
//!   `resets_at` of one window, under the unit the dialect states for it.
//! - A header the service did not send leaves its field empty: a missing header is not a zero.
//! - An empty header value counts as absent, and header names are matched case-insensitively.
//!
//! Constraints:
//! - These dialects have no endpoint of their own: they are read off the reply to a call this crate
//!   already made, which is why [`parse`] takes headers rather than a body.
//! - A reply carrying none of a dialect's headers reads as an `AccountState` with a warning,
//!   so "the
//!   service said nothing" is not mistaken for "there is nothing left".
//!
//! Trade-offs:
//! - `used` is left empty: it would have to be derived from a limit, and a request budget and a
//!   token budget are counted over windows that do not line up.
//! - Only the headers these services document are read, so a limit the same service sends under an
//!   undocumented name stays unread rather than guessed at.
//! - A reset that arrives as a duration (`1s`, `6m30s`) is kept as spelled in `resets_at`, beside
//!   the timestamps other services send: this shape does not turn one clock into another.
//! - Anthropic's input and output budgets stay apart rather than being summed, because the service
//!   limits them apart.

use crate::Error;
use crate::protocol::account_state::{AccountState, AccountStateProtocol, QuotaWindow};

/// One window a dialect reads out of a reply's headers.
struct HeaderWindow {
  /// Window id, in this crate's shape.
  id: &'static str,
  /// What the window's amounts count.
  unit: &'static str,
  /// Header carrying what is left.
  remaining: Option<&'static str>,
  /// Header carrying the allowance.
  limit: Option<&'static str>,
  /// Header carrying when the window resets.
  resets_at: Option<&'static str>,
}

/// One service's header vocabulary.
struct HeaderDialect {
  protocol: AccountStateProtocol,
  windows: &'static [HeaderWindow],
}

/// The header dialects, each as its own service documents it.
static DIALECTS: &[HeaderDialect] = &[
  HeaderDialect {
    protocol: AccountStateProtocol::AnthropicRatelimitHeaders,
    windows: &[
      HeaderWindow {
        id: "requests",
        unit: "requests",
        remaining: Some("anthropic-ratelimit-requests-remaining"),
        limit: Some("anthropic-ratelimit-requests-limit"),
        resets_at: Some("anthropic-ratelimit-requests-reset"),
      },
      HeaderWindow {
        id: "tokens",
        unit: "tokens",
        remaining: Some("anthropic-ratelimit-tokens-remaining"),
        limit: Some("anthropic-ratelimit-tokens-limit"),
        resets_at: Some("anthropic-ratelimit-tokens-reset"),
      },
      HeaderWindow {
        id: "input_tokens",
        unit: "tokens",
        remaining: Some("anthropic-ratelimit-input-tokens-remaining"),
        limit: Some("anthropic-ratelimit-input-tokens-limit"),
        resets_at: Some("anthropic-ratelimit-input-tokens-reset"),
      },
      HeaderWindow {
        id: "output_tokens",
        unit: "tokens",
        remaining: Some("anthropic-ratelimit-output-tokens-remaining"),
        limit: Some("anthropic-ratelimit-output-tokens-limit"),
        resets_at: Some("anthropic-ratelimit-output-tokens-reset"),
      },
    ],
  },
  HeaderDialect {
    protocol: AccountStateProtocol::OpenAiRatelimitHeaders,
    windows: &[
      HeaderWindow {
        id: "requests",
        unit: "requests",
        remaining: Some("x-ratelimit-remaining-requests"),
        limit: Some("x-ratelimit-limit-requests"),
        resets_at: Some("x-ratelimit-reset-requests"),
      },
      HeaderWindow {
        id: "tokens",
        unit: "tokens",
        remaining: Some("x-ratelimit-remaining-tokens"),
        limit: Some("x-ratelimit-limit-tokens"),
        resets_at: Some("x-ratelimit-reset-tokens"),
      },
    ],
  },
  HeaderDialect {
    protocol: AccountStateProtocol::GroqRatelimitHeaders,
    windows: &[
      HeaderWindow {
        id: "tokens",
        unit: "tokens",
        remaining: Some("x-ratelimit-remaining-tokens"),
        limit: Some("x-ratelimit-limit-tokens"),
        resets_at: Some("x-ratelimit-reset-tokens"),
      },
      HeaderWindow {
        id: "requests",
        unit: "requests",
        remaining: Some("x-ratelimit-remaining-requests"),
        limit: Some("x-ratelimit-limit-requests"),
        resets_at: Some("x-ratelimit-reset-requests"),
      },
    ],
  },
  HeaderDialect {
    protocol: AccountStateProtocol::CerebrasRatelimitHeaders,
    windows: &[
      HeaderWindow {
        id: "tokens",
        unit: "tokens",
        remaining: Some("x-ratelimit-remaining-tokens-minute"),
        limit: Some("x-ratelimit-limit-tokens-minute"),
        resets_at: Some("x-ratelimit-reset-tokens-minute"),
      },
      HeaderWindow {
        id: "requests",
        unit: "requests",
        remaining: Some("x-ratelimit-remaining-requests-day"),
        limit: Some("x-ratelimit-limit-requests-day"),
        resets_at: Some("x-ratelimit-reset-requests-day"),
      },
    ],
  },
  HeaderDialect {
    protocol: AccountStateProtocol::MistralRatelimitHeaders,
    windows: &[
      HeaderWindow {
        id: "requests",
        unit: "requests",
        remaining: Some("x-ratelimit-remaining-req-minute"),
        limit: Some("x-ratelimit-limit-req-minute"),
        resets_at: Some("x-ratelimit-reset-req-minute"),
      },
      HeaderWindow {
        id: "tokens",
        unit: "tokens",
        remaining: Some("x-ratelimit-remaining-tokens-minute"),
        limit: Some("x-ratelimit-limit-tokens-minute"),
        resets_at: Some("x-ratelimit-reset-tokens-minute"),
      },
    ],
  },
];

/// Reads a reply's headers for a known header dialect.
///
/// # Errors
///
/// Returns [`Error::Malformed`] for a protocol that is not read from headers.
pub fn parse(
  protocol: AccountStateProtocol,
  headers: &[(String, String)],
) -> Result<AccountState, Error> {
  let dialect = DIALECTS
    .iter()
    .find(|dialect| dialect.protocol == protocol)
    .ok_or_else(|| Error::Malformed(format!("`{protocol}` is not read from headers")))?;
  let mut quotas = Vec::new();
  for window in dialect.windows {
    let quota = QuotaWindow {
      id: window.id.to_owned(),
      name: None,
      unit: window.unit.to_owned(),
      used: None,
      limit: window.limit.and_then(|name| get_header_value(headers, name)),
      remaining: window.remaining.and_then(|name| get_header_value(headers, name)),
      used_percent: None,
      window: None,
      resets_at: window.resets_at.and_then(|name| get_header_value(headers, name)),
      reached: None,
      unlimited: None,
    };
    // A window whose headers all stayed away is not a window the service reported.
    if quota.limit.is_none() && quota.remaining.is_none() && quota.resets_at.is_none() {
      continue;
    }
    quotas.push(quota);
  }
  let mut warnings = Vec::new();
  if quotas.is_empty() {
    warnings.push(format!("this reply carried none of the {protocol} headers"));
  }
  Ok(AccountState {
    protocol,
    quotas,
    balances: Vec::new(),
    failure: None,
    warnings,
    availability: None,
    plan_type: None,
  })
}

/// Whether a protocol is read from a reply's headers.
pub(crate) fn is_header_dialect(protocol: AccountStateProtocol) -> bool {
  DIALECTS.iter().any(|dialect| dialect.protocol == protocol)
}

/// A header's value, trimmed: an empty header is a header the service did not send.
fn get_header_value(headers: &[(String, String)], name: &str) -> Option<String> {
  headers
    .iter()
    .find(|(key, _)| key.eq_ignore_ascii_case(name))
    .map(|(_, value)| value.trim())
    .filter(|value| !value.is_empty())
    .map(str::to_owned)
}
