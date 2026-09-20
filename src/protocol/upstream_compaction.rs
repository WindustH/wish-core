//! Giving a service a history to hold in place of the one that grew too long.
//!
//! A compaction call takes a conversation and hands back what stands in for it: the service's own
//! opaque payload, which goes back untouched on every later call instead of the messages it
//! replaced. It is a dimension of its own beside [`model_use`](crate::protocol::model_use), not a
//! model call: there are no tools, no output cap and no reasoning controls to set, there is no
//! stream to follow, and what comes back is the caller's next starting point rather than an answer.
//!
//! One wire has this call so far - the responses wire, whose platform form and Codex deployment
//! differ only in what rides beside the body - so this dimension is one renderer and one reader, and
//! a client speaking any other protocol refuses the call rather than sending something else.
//!
//! Whether a history is long enough to be worth compacting, and what becomes of the messages that
//! were replaced, are the caller's decisions; nothing here decides for them.

pub mod request;
pub mod response;

use crate::protocol::account_state::AccountState;
use crate::protocol::model_use::message::Conversation;
use crate::protocol::model_use::response::Usage;

/// The protocol kinds of upstream compaction: the wires that have this call at all.
///
/// Chosen when a client is built, beside the model-use protocol it pairs with, and refused there
/// when the pairing is not one the crate knows: an ask this wire cannot serve never reaches the
/// network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpstreamCompactionProtocol {
  /// The OpenAI Responses family's `/compact` call, served buffered.
  OpenAiResponses,
  /// A deployment that answers a compaction on the streamed call it always serves: the trigger
  /// rides the body, and the item standing in for the history arrives on the stream.
  OpenAiResponsesStreamed,
}

impl UpstreamCompactionProtocol {
  /// The name this protocol is known by in text.
  pub const fn get_id(self) -> &'static str {
    match self {
      UpstreamCompactionProtocol::OpenAiResponses => "openai_responses",
      UpstreamCompactionProtocol::OpenAiResponsesStreamed => "openai_responses_streamed",
    }
  }
}

/// Why an upstream compaction cannot be asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
  /// The wire has no compaction call of any kind.
  NoCall,
}

impl Unsupported {
  /// The reason in the words a caller reads back.
  pub const fn get_text(self) -> &'static str {
    match self {
      Unsupported::NoCall => "has no upstream compaction call",
    }
  }
}

/// One call that asks a service to stand in for a conversation.
pub struct UpstreamCompactionRequest {
  /// The model that does the compacting.
  pub model: String,
  /// The history to compact, in the order it was sent.
  pub conversation: Conversation,
}

/// What a service handed back in place of a conversation.
pub struct UpstreamCompaction {
  /// The items to start the next call with, in the order they have to travel: the compacted history,
  /// preceded by whatever the service handed back verbatim beside it.
  pub conversation: Conversation,
  /// What the compaction itself cost, as the service reported it.
  pub usage: Usage,
  /// What the reply said about the account_state behind the key, when the endpoint names a reading.
  pub account_state: Option<AccountState>,
  /// What the reader could not represent, one line each; a caller that cares checks this before
  /// trusting the conversation it got back.
  pub warnings: Vec<String>,
}
