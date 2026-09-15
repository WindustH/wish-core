//! The call as the caller describes it, and the renderers that put it on a wire.
//!
//! One [`Request`] is what a caller wants to say, independent of any wire: every field is an axis a
//! protocol can honor, reject or ignore, and each renderer says which it does. They live one module
//! per protocol under this one, so the shape and its translations are read together; a protocol with
//! no renderer of its own shares the wire of one that has, differing only in endpoint and
//! authentication.

pub mod anthropic_messages;
pub mod bedrock_converse;
pub mod google_generate_content;
pub mod google_interactions;
pub mod mistral_conversations;
pub mod openai_chat;
pub mod openai_responses;

use crate::protocol::error::Error;

use super::message::Conversation;
use super::tool::Tool;

/// How the model should pick among the tools it was given.
#[derive(Clone, Copy, PartialEq)]
pub enum ToolChoice {
  /// The model decides; the wire's default.
  Auto,
  /// No tool may be called this turn. A wire with no spelling for it rejects the request.
  None,
  /// At least one tool must be called.
  Required,
}

/// How much the model should think.
///
/// Every axis is optional: `None` leaves that decision to the service default, and a wire that has
/// no place for an axis says so when it renders the request.
#[derive(Clone, Default, PartialEq)]
pub struct ReasoningConfig {
  /// Explicit on/off switch; a wire that cannot spell "off" says so when it renders the request.
  pub enabled: Option<bool>,
  /// Depth tier, spelled the way the model spells it: every service names its own set of tiers, so
  /// an effort wire passes the word through. A wire that steers thinking with a token budget has no
  /// tier word of its own and takes the `tier_budget` preset instead.
  pub effort: Option<String>,
  /// Whether the service should also hand back a readable summary of its thoughts: the part that is
  /// meant to be shown to a reader, as opposed to the raw reasoning the model itself replays.
  pub summary: Option<ReasoningSummary>,
}

/// What one word of the budget preset asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TierBudget {
  /// Extended thinking, with this many tokens to spend.
  Tokens(u64),
  /// Thinking whose depth the model picks itself.
  Adaptive,
}

/// The preset a budget wire reads a depth tier from.
///
/// The wires that steer thinking by token budget share one preset instead of naming tiers of their
/// own: each word is either a token budget or `adaptive`. A word outside the preset has no control
/// to become and is reported as a build error.
pub(crate) fn tier_budget(effort: &str) -> Result<TierBudget, Error> {
  match effort {
    "low" => Ok(TierBudget::Tokens(1024)),
    "medium" => Ok(TierBudget::Tokens(4096)),
    "high" => Ok(TierBudget::Tokens(16384)),
    "max" => Ok(TierBudget::Tokens(32768)),
    "adaptive" => Ok(TierBudget::Adaptive),
    other => Err(Error::Build(format!(
      "`{other}` is not one of the budget tiers `low`, `medium`, `high`, `max` or `adaptive`"
    ))),
  }
}

/// How much room the answer keeps above the thinking budget.
pub(crate) const ANSWER_HEADROOM: u64 = 4096;

/// Whether the service should also hand back a readable summary of its thinking.
#[derive(Clone, Copy, PartialEq)]
pub enum ReasoningSummary {
  /// Ask for one.
  Auto,
  /// Do not ask for one.
  None,
}

/// Which caching this prompt asks for.
///
/// The two fields are the two ways the wires cache, and each wire takes whichever one it has a place
/// for: the key names the prefix for the wires that route on a key, the flag asks the wires that
/// cache at an explicit marker to end their cached prefix at the prompt tail. Wires that cache
/// implicitly take neither.
#[derive(Clone, PartialEq)]
pub struct PromptCache {
  /// The name the keyed wires route this prefix by.
  pub key: Option<String>,
  /// Whether a marker-caching wire should mark the prompt tail.
  pub breakpoints: bool,
}

/// One call, as the caller describes it, independent of any wire.
///
/// Every field is an axis a protocol can honor, reject or ignore, and the renderers say which.
pub struct Request {
  /// The service's model id.
  pub model: String,
  /// The conversation, oldest message first.
  pub conversation: Conversation,
  /// Tools the model may call; empty means a plain answer.
  pub tools: Vec<Tool>,
  /// How the model should pick among `tools`; `None` leaves it to the service.
  pub tool_choice: Option<ToolChoice>,
  /// Cap on answer tokens, on wires that take one.
  pub max_output_tokens: Option<u64>,
  /// How much the model should think (see [`ReasoningConfig`]).
  pub reasoning: Option<ReasoningConfig>,
  /// Which caching to ask for (see [`PromptCache`]).
  pub cache: Option<PromptCache>,
}
