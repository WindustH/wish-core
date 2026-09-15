//! The conversation, as the model sees it.
//!
//! A `Conversation` is a flat list of messages, oldest first; folding it into the turn structure a
//! wire demands is the model call's job. `Reasoning` is the variant to get right: `plaintext` is the
//! readable text that has to go back, `signature` proves it, `ciphertext` is a payload that stands
//! in for it, and `display` is the one field meant for a reader - it never goes back upstream.

use serde_json::Value;

/// The conversation in order, oldest message first.
pub type Conversation = Vec<Message>;

/// One piece of content inside a message.
#[derive(Debug)]
pub enum ContentBlock {
  /// Plain text.
  Text { text: String },
  /// An inline image, base64 encoded, with the media type it was encoded from.
  Image { mime_type: String, data_base64: String },
}

/// One message in the conversation, independent of any wire's role names.
#[derive(Debug)]
pub enum Message {
  /// Instructions that outrank the conversation, on wires that have a place for them.
  System { content: Vec<ContentBlock> },
  /// Instructions from the integrating application: a role most wires fold into `System` or `User`.
  Developer { content: Vec<ContentBlock> },
  /// A message from the caller.
  User { content: Vec<ContentBlock> },
  /// A message the model produced.
  Assistant { content: Vec<ContentBlock> },
  /// Thinking the model produced, which the service expects to see again on the next turn.
  Reasoning {
    /// The readable thinking text the wire gave and gets back: `thinking`, `reasoningText.text`,
    /// `reasoning_content`, `reasoning_text` content, a thought part.
    plaintext: String,
    /// The thinking text meant for a reader, which no wire gets back: the service's own summary
    /// where it has one, otherwise a copy of `plaintext`.
    display: String,
    /// The proof that rides with `plaintext` (`signature`, `thoughtSignature`); empty when the wire
    /// returned none.
    signature: String,
    /// A payload that stands in for `plaintext` rather than proving it: a redacted block's blob, an
    /// encrypted reasoning item. Filled means the block is redacted or encrypted, so the wire shape
    /// follows from which of these fields carries something.
    ciphertext: String,
  },
  /// One tool call the model asked for; `arguments` is the JSON object it was called with.
  ToolUse { call_id: String, name: String, arguments: Value },
  /// The result of one tool call, paired by `call_id`; `content` travels as JSON when it is an
  /// object and is stringified otherwise.
  ToolResult { call_id: String, name: String, content: Value },
  /// A compacted history standing in for the messages it replaced: the service produced it, the
  /// payload is opaque, and it goes back exactly as it came, ahead of whatever follows it.
  ///
  /// Only the responses wire carries one; every other wire refuses the conversation rather than
  /// quietly sending the rest of it, which would look like a history that never happened.
  UpstreamCompaction {
    /// The service's name for this compaction, when it gave one.
    id: Option<String>,
    /// The encrypted history, sent back verbatim.
    encrypted_content: String,
  },
}
