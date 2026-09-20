//! The contract layer: what the wires say, where a call goes, and how failures are named.
//!
//! Every request is the same product: what a wire says, rendered by one of the payload trees
//! and handed over as a draft, and how it is reached - [`outbound`], the target, the auth plan and
//! the account material that join a draft into one call. The payload trees are [`model_use`], a
//! call as our caller describes it - the conversation, the tools, the request, the response and
//! the stream - beside the per-protocol shapes that translate each of them, [`upstream_compaction`], the
//! call that asks a service to stand in for a conversation that grew too long, [`account_state`], the
//! bodies a service reports about the key that reached it, and [`model_list`], the pages it lists
//! its models in. [`wire`] is the attempt contract a transport implements, and [`error`] with
//! [`http_error`] is the failure vocabulary every layer speaks.
//!
//! What the reading conventions share lives here, at the top.

pub mod account_state;
pub mod error;
pub mod http_error;
pub mod model_list;
pub mod model_use;
pub mod outbound;
pub mod upstream_compaction;
pub mod wire;

pub use model_use::message::{ContentBlock, Conversation, Message};
pub use model_use::request::{PromptCache, ReasoningConfig, ReasoningSummary, Request, ToolChoice};
pub use model_use::response::{Response, StopReason, Usage};
pub use model_use::stream::{BlockKind, StreamAccumulator, StreamEvent};
pub use model_use::tool::Tool;
pub use upstream_compaction::{UpstreamCompaction, UpstreamCompactionRequest};

use serde_json::Value;

/// A value exactly as the service wrote it, whether it wrote a string or a number.
///
/// Both readings keep one rule: what the service said is what the caller sees. A balance re-encoded
/// through a float can lose the cent that matters, and a zero that was invented is worse than a
/// missing value.
pub(crate) fn read_scalar_text(value: &Value) -> Option<String> {
  match value {
    Value::String(text) => Some(text.clone()),
    Value::Number(number) => Some(number.to_string()),
    _ => None,
  }
}

/// A share a service reported as a ratio (`0.22`), spelled as the percentage other services report
/// (`22`).
///
/// The point moves rather than the number being multiplied, so no digit is invented and none is
/// lost to a float: `0.125` reads `12.5`, `1` reads `100`, `0.0` reads `0`. A value that is not a
/// decimal number reads as nothing at all.
pub(crate) fn convert_ratio_to_percent(ratio: &str) -> Option<String> {
  let (sign, digits) = match ratio.strip_prefix('-') {
    Some(magnitude) => ("-", magnitude),
    None => ("", ratio),
  };
  let (whole, fraction) = match digits.split_once('.') {
    Some((whole, fraction)) => (whole, fraction),
    None => (digits, ""),
  };
  if whole.is_empty() && fraction.is_empty() {
    return None;
  }
  if !whole.chars().chain(fraction.chars()).all(|digit| digit.is_ascii_digit()) {
    return None;
  }
  let mut fraction = fraction.to_owned();
  while fraction.len() < 2 {
    fraction.push('0');
  }
  let (moved, rest) = fraction.split_at(2);
  let integer = match format!("{whole}{moved}").trim_start_matches('0') {
    "" => "0".to_owned(),
    trimmed => trimmed.to_owned(),
  };
  let mut percent = String::from(sign);
  percent.push_str(&integer);
  let rest = rest.trim_end_matches('0');
  if !rest.is_empty() {
    percent.push('.');
    percent.push_str(rest);
  }
  Some(percent)
}
