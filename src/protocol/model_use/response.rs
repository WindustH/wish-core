//! What came back: the messages the model produced, why it stopped, what it cost, and what the
//! reply said about the account behind the key.
//!
//! A [`Response`] is normalized across the dialects - reasoning, text and tool calls, each as its
//! own message - and each reader below turns one wire's body into it, one module per protocol. The
//! account reading is the one part that comes from the reply's head rather than from its body: a
//! service reports what a call left in the account only on a call it really served, so it is read
//! by the layer that holds the whole reply and filled in beside the decoded body.

pub mod anthropic_messages;
pub mod bedrock_converse;
pub mod google_generate_content;
pub mod google_interactions;
pub mod mistral_conversations;
pub mod openai_chat;
pub mod openai_responses;

use super::message::Message;
use crate::protocol::account_state::AccountState;

/// One decoded reply, normalized across the dialects.
#[derive(Debug)]
pub struct Response {
  /// What the model produced this turn, in the order it produced it: reasoning, text and tool calls
  /// each as their own message.
  pub messages: Vec<Message>,
  /// Why the turn ended.
  pub stop_reason: StopReason,
  /// The token counts the service reported.
  pub usage: Usage,
  /// What the reply said about the account behind the key, when the endpoint named an account
  /// protocol: a rate limit is reported on the call that was served, never on a request of its own.
  pub account_state: Option<AccountState>,
}

/// Why the model stopped, normalized across the dialects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
  /// The model finished its answer.
  Stop,
  /// The model wants tool results before it continues; `messages` holds the calls.
  ToolUse,
  /// The service could not parse the tool call the model produced.
  MalformedToolUse,
  /// The output token cap was reached.
  MaxOutputLengthExceeded,
  /// The conversation no longer fits the model's context window, so the turn was cut short: asking
  /// for more output would not help, the input has to shrink.
  ContextLengthExceeded,
  /// A cap on generated messages ended the turn, rather than the model.
  MaxMessages,
  /// The answer was cut short because the turn was steered.
  Steered,
  /// The service refused to answer: safety, policy or recitation.
  ContentFilter,
  /// The caller cancelled the call.
  Cancelled,
  /// The wire carried a reason this crate does not model.
  Unknown,
}

/// The token counts for one call.
///
/// Every field is optional because the dialects report different subsets; a missing count is not a
/// zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
  /// Input tokens billed at the full rate.
  pub input_tokens: Option<u64>,
  /// Input tokens served from the service's cache, billed at a discount.
  pub cached_input_tokens: Option<u64>,
  /// Input tokens written into the cache, billed at a premium.
  pub cache_write_input_tokens: Option<u64>,
  /// Answer tokens.
  pub output_tokens: Option<u64>,
  /// The part of `output_tokens` spent thinking, where the service separates the two.
  pub reasoning_tokens: Option<u64>,
  /// The service's own total, where it reports one.
  pub total_tokens: Option<u64>,
}
