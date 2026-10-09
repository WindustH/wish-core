//! The call as the caller describes it, and the renderers that put it on a wire.
//!
//! One [`Request`] is what a caller wants to say, independent of any wire: every field is an axis a
//! protocol can honor, reject or ignore, and each renderer says which it does. They live one module
//! per protocol under this one, so the shape and its translations are read together; a protocol with
//! no renderer of its own shares the wire of one that has, differing only in endpoint and
//! authentication.
//!
//! What several renderers agree on lives here too: the leading run of instructions the wires with a
//! top-level instruction field lift out, the turns of the wires whose roles strictly alternate, and
//! the thinking controls of the wires that carry Claude's parameters. Each renderer keeps its own
//! wording for what it refuses.

pub mod anthropic_messages;
pub mod bedrock_converse;
pub mod google_generate_content;
pub mod google_interactions;
pub mod mistral_conversations;
pub mod openai_chat;
pub mod openai_responses;

use serde_json::{Map, Value, json};

use crate::protocol::error::Error;

use super::message::{ContentBlock, Conversation, Message};
use super::tool::Tool;

/// How the model should pick among the tools it was given.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq)]
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ReasoningConfig {
  /// Explicit on/off switch; a wire that cannot spell "off" says so when it renders the request.
  pub enabled: Option<bool>,
  /// Depth tier, spelled the way the model spells it: every service names its own set of tiers, so
  /// an effort wire passes the word through. A model that steers thinking with a token budget has
  /// no tier word of its own and takes the `resolve_tier_budget` preset instead.
  pub effort: Option<String>,
  /// Whether the service should also hand back a readable summary of its thoughts: the part that is
  /// meant to be shown to a reader, as opposed to the raw reasoning the model itself replays.
  pub summary: Option<ReasoningSummary>,
}

/// What one word of the budget preset asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TierBudget {
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
fn resolve_tier_budget(effort: &str) -> Result<TierBudget, Error> {
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

/// Claude's own thinking controls, shared by the wires that carry Claude's parameters: the Messages
/// wire itself and the Converse wire's bridge. The fields come back to be placed in the body, or
/// in whatever carries the model's own fields there.
///
/// Claude has taken two generations of control. The models before Claude Opus 4.6 and Sonnet 4.6
/// budget their thinking, so a depth tier selects the preset (extended thinking with its budget, or
/// adaptive thinking). Every later model thinks adaptively, takes the tier word itself as
/// `output_config.effort`, and refuses a budget. On both, `enabled` alone selects adaptive thinking
/// and the off state is omission, since the object has no `disabled` member. A config that both
/// disables thinking and gives it a tier is the caller's to refuse first, in its own words.
pub(crate) fn render_claude_thinking(
  model: &str,
  config: &ReasoningConfig,
) -> Result<Map<String, Value>, Error> {
  let mut fields = Map::new();
  match (config.enabled, config.effort.as_deref()) {
    (Some(false) | None, None) => {}
    (Some(true), None) => {
      fields.insert("thinking".into(), json!({"type": "adaptive"}));
    }
    (_, Some(effort)) if budgets_thinking(model) => {
      let thinking = match resolve_tier_budget(effort)? {
        TierBudget::Tokens(budget_tokens) => {
          json!({"type": "enabled", "budget_tokens": budget_tokens})
        }
        TierBudget::Adaptive => json!({"type": "adaptive"}),
      };
      fields.insert("thinking".into(), thinking);
    }
    (_, Some(effort)) => {
      fields.insert("thinking".into(), json!({"type": "adaptive"}));
      if effort != "adaptive" {
        fields.insert("output_config".into(), json!({"effort": effort.to_lowercase()}));
      }
    }
  }
  Ok(fields)
}

/// Whether a Claude model budgets its thinking: the generations before Claude Opus 4.6 and Sonnet
/// 4.6. The list is closed, since every later model takes an effort instead, and it is matched
/// anywhere in the id, so dated snapshots and Bedrock's prefixed ids read the same.
fn budgets_thinking(model: &str) -> bool {
  const BUDGETED: &[&str] = &[
    "claude-3",
    "claude-opus-4-0",
    "claude-opus-4-1",
    "claude-opus-4-2025",
    "claude-opus-4-5",
    "claude-sonnet-4-0",
    "claude-sonnet-4-2025",
    "claude-sonnet-4-5",
    "claude-haiku-4-5",
  ];
  BUDGETED.iter().any(|family| model.contains(family))
}

/// How much room the answer keeps above the thinking budget.
const ANSWER_HEADROOM: u64 = 4096;

/// The output cap a Claude thinking budget leaves room under.
///
/// The model refuses to answer at or below the tokens it thinks with, so a cap that tight is raised
/// to the budget plus [`ANSWER_HEADROOM`] instead of failing the call.
pub(crate) fn lift_cap_above_budget(cap: u64, budget_tokens: u64) -> u64 {
  if cap > budget_tokens { cap } else { budget_tokens + ANSWER_HEADROOM }
}

/// The `thinking.type` on/off switch several vendors patch into the Anthropic and chat wires.
pub(crate) fn render_thinking_switch(enabled: bool) -> Value {
  json!({"type": if enabled { "enabled" } else { "disabled" }})
}

/// Whether the service should also hand back a readable summary of its thinking.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq)]
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
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
pub struct PromptCache {
  /// The name the keyed wires route this prefix by.
  pub key: Option<String>,
  /// Whether a marker-caching wire should mark the prompt tail.
  pub breakpoints: bool,
}

/// One call, as the caller describes it, independent of any wire.
///
/// Every field is an axis a protocol can honor, reject or ignore, and the renderers say which.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Request {
  /// Request incremental events instead of a buffered response. The protocol renders this
  /// choice into body fields or the endpoint path, and the client selects the matching reader.
  pub stream: bool,
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

/// The leading run of instructions - the `System` and `Developer` messages a conversation opens
/// with - as the text blocks of each message, and the conversation that remains after it.
///
/// The wires with a top-level instruction field take the run there, each joining it by its own
/// rule, and an instruction past the run is part of the conversation. Such a field only takes text,
/// so a block of any other kind is refused in the wire's own words, `refusal`.
pub(crate) fn split_leading_instructions<'a>(
  conversation: &'a [Message],
  refusal: &str,
) -> Result<(Vec<Vec<&'a str>>, &'a [Message]), Error> {
  let mut run = Vec::new();
  let mut rest = conversation;
  while let [Message::System { content, .. } | Message::Developer { content, .. }, tail @ ..] = rest
  {
    run.push(collect_instruction_text(content, refusal)?);
    rest = tail;
  }
  Ok((run, rest))
}

/// The text blocks of one instruction message, in order; any other block is refused with
/// `refusal`, because instructions only travel as text.
pub(crate) fn collect_instruction_text<'a>(
  content: &'a [ContentBlock],
  refusal: &str,
) -> Result<Vec<&'a str>, Error> {
  content
    .iter()
    .map(|block| match block {
      ContentBlock::Text { text } => Ok(text.as_str()),
      _ => Err(Error::Build(refusal.to_owned())),
    })
    .collect()
}

/// The turns of a wire whose roles strictly alternate (Anthropic, Google, Converse), built one push
/// at a time: a push lands on the turn before it when the role is the same, so the conversation's
/// adjacent same-role messages become one turn, and an empty push adds nothing.
#[derive(Default)]
pub(crate) struct AlternatingTurns {
  turns: Vec<(&'static str, Vec<Value>)>,
}

impl AlternatingTurns {
  pub(crate) fn push(&mut self, role: &'static str, mut blocks: Vec<Value>) {
    if blocks.is_empty() {
      return;
    }
    if let Some((last_role, last_blocks)) = self.turns.last_mut()
      && *last_role == role
    {
      last_blocks.append(&mut blocks);
      return;
    }
    self.turns.push((role, blocks));
  }

  /// The turns as wire objects, `{"role", <blocks_key>: [...]}`, refused with `refusal` when the
  /// first one is not a `user` turn: a wire that alternates opens with the caller.
  pub(crate) fn finish_from_user(
    self,
    blocks_key: &str,
    refusal: &str,
  ) -> Result<Vec<Value>, Error> {
    match self.turns.first() {
      Some((role, _)) if *role != "user" => Err(Error::Build(refusal.to_owned())),
      _ => Ok(self.finish(blocks_key)),
    }
  }

  /// The turns as wire objects, `{"role", <blocks_key>: [...]}`, whichever role opens them.
  pub(crate) fn finish(self, blocks_key: &str) -> Vec<Value> {
    self.turns.into_iter().map(|(role, blocks)| json!({"role": role, blocks_key: blocks})).collect()
  }
}
