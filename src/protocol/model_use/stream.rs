//! Streamed calls: the events a protocol's decoder produces, and the accumulator that rebuilds a
//! response from them.
//!
//! A protocol decoder assembles nothing: it numbers blocks, opens them with their kind, streams
//! deltas into them and closes them. Numbering belongs to the protocol (chat completions number tool
//! calls, anthropic numbers content blocks, and a protocol without numbers of its own assigns them),
//! and the accumulator is the single place that turns those blocks back into our flat message list:
//! text blocks keep their boundaries inside one assistant message, a reasoning block becomes its own
//! message, a tool call becomes its own.
//!
//! The contract it enforces is the one a decoder is most likely to break: a block opens once, deltas
//! only land on the block they name, a tool block has an id and a name by the time it closes, and
//! nothing follows the terminal event. Violations are protocol errors, because from outside a stream
//! that breaks its own shape is indistinguishable from a malformed response body.
//!
//! The decoders live one module per protocol under this one, and [`StreamDecoder`] is the single
//! place that picks between them, so a client names its protocol once and never matches again.
//!
//! Not every event is part of the answer: a stream may carry a reading beside it, such as the quota
//! a Codex account_state has left. That becomes [`StreamEvent::Account`] - the accumulator keeps the last
//! one it sees beside the messages, starting from the one
//! [`StreamAccumulator::with_account_state`] was handed for the reply head.

pub mod anthropic_messages;
pub mod bedrock_converse;
pub mod google_generate_content;
pub mod google_interactions;
pub mod mistral_conversations;
pub mod openai_chat;
pub mod openai_responses;

use std::collections::{BTreeMap, VecDeque};

use serde_json::json;

use crate::protocol::account_state::AccountState;
use crate::protocol::error::Error;

use super::message::{ContentBlock, Message};
use super::response::{Response, StopReason, Usage};

/// Which protocol decodes a stream.
pub enum StreamDecoder {
  AnthropicMessages(anthropic_messages::Decoder),
  BedrockConverse(bedrock_converse::Decoder),
  GoogleGenerateContent(google_generate_content::Decoder),
  GoogleInteractions(google_interactions::Decoder),
  MistralConversations(mistral_conversations::Decoder),
  OpenAiChat(openai_chat::Decoder),
  OpenAiResponses(openai_responses::Decoder),
}

impl StreamDecoder {
  /// Feeds one SSE record: its `event:` name (usually redundant with the payload's `type`) and
  /// its `data:` payload.
  pub fn feed(&mut self, event: Option<&str>, data: &str) -> Result<Vec<StreamEvent>, Error> {
    match self {
      StreamDecoder::AnthropicMessages(decoder) => decoder.feed(event, data),
      StreamDecoder::BedrockConverse(decoder) => decoder.feed(event, data),
      StreamDecoder::GoogleGenerateContent(decoder) => decoder.feed(event, data),
      StreamDecoder::GoogleInteractions(decoder) => decoder.feed(event, data),
      StreamDecoder::MistralConversations(decoder) => decoder.feed(event, data),
      StreamDecoder::OpenAiChat(decoder) => decoder.feed(event, data),
      StreamDecoder::OpenAiResponses(decoder) => decoder.feed(event, data),
    }
  }

  /// Synthesizes the terminal events of a body that ended without one. Only a protocol whose wire
  /// has no terminal marker (the body ending is the terminal) produces anything here; the others
  /// stay silent, and their accumulator then reports the missing stop event.
  pub fn finish(&mut self) -> Result<Vec<StreamEvent>, Error> {
    match self {
      StreamDecoder::BedrockConverse(decoder) => decoder.finish(),
      StreamDecoder::GoogleGenerateContent(decoder) => decoder.finish(),
      _ => Ok(Vec::new()),
    }
  }
}

/// What a streamed block carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
  Text,
  Reasoning,
  ToolUse,
}

/// One normalized event from a protocol's stream decoder.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
  /// A block opened at `index`; deltas for it may follow.
  BlockStart { index: u32, kind: BlockKind },
  /// Text appended to a text block.
  TextDelta { index: u32, delta: String },
  /// Visible reasoning text appended to a reasoning block.
  ReasoningDelta { index: u32, delta: String },
  /// Reasoning signature (`signature`, `thoughtSignature`) appended as it arrives.
  ReasoningSignatureDelta { index: u32, signature: String },
  /// Reasoning ciphertext (`redacted_thinking.data`, `encrypted_content`, a redacted block) appended
  /// as it arrives.
  ReasoningCiphertextDelta { index: u32, ciphertext: String },
  /// Argument JSON fragment for a tool block. `call_id` and `name` arrive at least once before the
  /// block closes.
  ToolUseDelta { index: u32, call_id: Option<String>, name: Option<String>, arguments: String },
  /// The block is complete.
  BlockEnd { index: u32 },
  /// Cumulative usage, replacing what was reported before.
  Usage(Usage),
  /// An account_state reading the stream itself carried, replacing whatever the reply head reported.
  Account(AccountState),
  /// An item the protocol handed over whole rather than streaming it in parts, at its own place in
  /// the index sequence: the opaque item a compacted history travels as.
  UpstreamCompaction { index: u32, id: Option<String>, encrypted_content: String },
  /// The response is over. Terminal.
  Stop(StopReason),
}

/// Rebuilds a [`Response`] from events, holding the stream to the contract above.
#[derive(Default)]
pub struct StreamAccumulator {
  blocks: BTreeMap<u32, Block>,
  /// Whole items, at the index they were handed over at, beside the blocks.
  items: BTreeMap<u32, Message>,
  usage: Option<Usage>,
  stop_reason: Option<StopReason>,
  account_state: Option<AccountState>,
}

/// One block while it is being streamed.
struct Block {
  kind: BlockKind,
  closed: bool,
  text: String,
  plaintext: String,
  signature: String,
  ciphertext: String,
  call_id: Option<String>,
  name: Option<String>,
  arguments: String,
}

impl Block {
  fn new(kind: BlockKind) -> Self {
    Self {
      kind,
      closed: false,
      text: String::new(),
      plaintext: String::new(),
      signature: String::new(),
      ciphertext: String::new(),
      call_id: None,
      name: None,
      arguments: String::new(),
    }
  }
}

impl StreamAccumulator {
  pub fn new() -> Self {
    Self::default()
  }

  /// Carries the account_state reading the opening reply held into the rebuilt [`Response`].
  ///
  /// That reading arrives with the reply head, before the first event, so it is handed over here
  /// rather than streamed; `None` leaves [`Response::account_state`] empty.
  #[must_use]
  pub fn with_account_state(mut self, account_state: Option<AccountState>) -> Self {
    self.account_state = account_state;
    self
  }

  /// Takes one event, or reports the contract violation it caused.
  pub fn feed(&mut self, event: StreamEvent) -> Result<(), Error> {
    if self.stop_reason.is_some() {
      return Err(violation("event after the stop event"));
    }
    match event {
      StreamEvent::BlockStart { index, kind } => self.open(index, kind),
      StreamEvent::TextDelta { index, delta } => {
        self.block_mut(index, BlockKind::Text)?.text.push_str(&delta);
        Ok(())
      }
      StreamEvent::ReasoningDelta { index, delta } => {
        self.block_mut(index, BlockKind::Reasoning)?.plaintext.push_str(&delta);
        Ok(())
      }
      StreamEvent::ReasoningSignatureDelta { index, signature } => {
        self.block_mut(index, BlockKind::Reasoning)?.signature.push_str(&signature);
        Ok(())
      }
      StreamEvent::ReasoningCiphertextDelta { index, ciphertext } => {
        self.block_mut(index, BlockKind::Reasoning)?.ciphertext.push_str(&ciphertext);
        Ok(())
      }
      StreamEvent::ToolUseDelta { index, call_id, name, arguments } => {
        let block = self.block_mut(index, BlockKind::ToolUse)?;
        if call_id.is_some() {
          block.call_id = call_id;
        }
        if name.is_some() {
          block.name = name;
        }
        block.arguments.push_str(&arguments);
        Ok(())
      }
      StreamEvent::BlockEnd { index } => self.close(index),
      StreamEvent::Usage(usage) => {
        self.usage = Some(usage);
        Ok(())
      }
      StreamEvent::Account(account_state) => {
        self.account_state = Some(account_state);
        Ok(())
      }
      StreamEvent::UpstreamCompaction { index, id, encrypted_content } => {
        self.items.insert(index, Message::UpstreamCompaction { id, encrypted_content });
        Ok(())
      }
      StreamEvent::Stop(reason) => {
        self.stop_reason = Some(reason);
        Ok(())
      }
    }
  }

  /// Consumes the accumulator and returns the assembled response.
  ///
  /// Messages leave in index order, which is the order the protocol numbered them rather than the
  /// order they happened to close in; a whole item takes the place its index gives it among the
  /// blocks.
  pub fn finish(self) -> Result<Response, Error> {
    let stop_reason =
      self.stop_reason.ok_or_else(|| violation("stream ended without a stop event"))?;
    let mut messages: Vec<Message> = Vec::new();
    let mut items: VecDeque<(u32, Message)> = self.items.into_iter().collect();
    for (index, block) in self.blocks {
      // A whole item handed over before this block keeps the place its index gives it.
      while items.front().is_some_and(|(at, _)| *at < index) {
        let (_, item) = items.pop_front().expect("the front was just seen");
        messages.push(item);
      }
      if !block.closed {
        return Err(violation(&format!("stream ended with block {index} still open")));
      }
      match block.kind {
        BlockKind::Text => match messages.last_mut() {
          Some(Message::Assistant { content }) => {
            content.push(ContentBlock::Text { text: block.text });
          }
          _ => messages
            .push(Message::Assistant { content: vec![ContentBlock::Text { text: block.text }] }),
        },
        BlockKind::Reasoning => {
          messages.push(Message::Reasoning {
            plaintext: block.plaintext.clone(),
            display: block.plaintext,
            signature: block.signature,
            ciphertext: block.ciphertext,
          });
        }
        BlockKind::ToolUse => {
          let call_id = block
            .call_id
            .ok_or_else(|| violation(&format!("tool block {index} closed without a call id")))?;
          let name = block
            .name
            .ok_or_else(|| violation(&format!("tool block {index} closed without a name")))?;
          let arguments = if block.arguments.trim().is_empty() {
            json!({})
          } else {
            serde_json::from_str(&block.arguments).map_err(|_| {
              Error::Malformed(format!(
                "arguments of streamed tool block {index} are not valid JSON"
              ))
            })?
          };
          messages.push(Message::ToolUse { call_id, name, arguments });
        }
      }
    }
    messages.extend(items.into_iter().map(|(_, item)| item));
    Ok(Response {
      messages,
      stop_reason,
      usage: self.usage.unwrap_or_default(),
      account_state: self.account_state,
    })
  }

  fn open(&mut self, index: u32, kind: BlockKind) -> Result<(), Error> {
    if self.blocks.contains_key(&index) {
      return Err(violation(&format!("block {index} is opened twice")));
    }
    self.blocks.insert(index, Block::new(kind));
    Ok(())
  }

  fn block_mut(&mut self, index: u32, kind: BlockKind) -> Result<&mut Block, Error> {
    let block = self
      .blocks
      .get_mut(&index)
      .ok_or_else(|| violation(&format!("event for block {index} before it opened")))?;
    if block.kind != kind {
      return Err(violation(&format!(
        "{kind:?} event for block {index}, which is {:?}",
        block.kind
      )));
    }
    if block.closed {
      return Err(violation(&format!("event for block {index} after it closed")));
    }
    Ok(block)
  }

  fn close(&mut self, index: u32) -> Result<(), Error> {
    let block = self
      .blocks
      .get_mut(&index)
      .ok_or_else(|| violation(&format!("block {index} closed before it opened")))?;
    if block.closed {
      return Err(violation(&format!("block {index} is closed twice")));
    }
    if block.kind == BlockKind::ToolUse {
      if block.call_id.is_none() {
        return Err(violation(&format!("tool block {index} closed without a call id")));
      }
      if block.name.is_none() {
        return Err(violation(&format!("tool block {index} closed without a name")));
      }
    }
    block.closed = true;
    Ok(())
  }
}

/// A protocol decoder broke the stream contract.
fn violation(message: &str) -> Error {
  Error::Malformed(format!("stream contract: {message}"))
}
