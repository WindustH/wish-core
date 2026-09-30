//! Interrupted output and its conservative projection into replayable protocol messages.
//!
//! A stream that did not end normally leaves blocks behind in every state: complete, cut short, or
//! never closed. Each keeps its raw content for inspection beside a [`ReplayDisposition`], and only
//! the replayable ones become the context fragment a caller can append - tool calls paired with the
//! results their execution state allows, and reasoning only where the producing protocol's rules
//! ([`ModelUseProtocol::get_reasoning_replay`]) say it can go back.
use super::{Block, BlockKind, ContentBlock, Message, StopReason, StreamAccumulator, Usage};
use crate::protocol::account_state::AccountState;
use crate::protocol::model_use::tool::parse_tool_arguments;
use crate::protocol::model_use::{ModelUseProtocol, ReasoningReplay};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayDisposition {
  Replayable,
  /// The wire has not confirmed the block's completion.
  Incomplete,
  /// A complete reasoning block has no replayable content, lacks required material, or uses
  /// a protocol configuration that does not allow reasoning replay.
  NotReplayable,
  InvalidToolCall,
  /// A signature-only item has no replayable following part to attach to.
  MissingSignedPart,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum PartialContent {
  Message(Message),
  /// Raw arguments are retained for inspection even when not valid JSON. Never executable.
  ToolCall {
    call_id: Option<String>,
    name: Option<String>,
    arguments: String,
  },
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct PartialBlock {
  pub index: u32,
  pub closed: bool,
  pub content: PartialContent,
  pub replay: ReplayDisposition,
}

/// Facts supplied by the owner, never inferred from the shape of model output.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolExecutionState {
  NotStarted,
  /// Some work may have been dispatched; no trustworthy execution outcome is available.
  MayHaveStarted,
}

/// Why reading ended. A local interruption is separate from the upstream's stop reason.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum StreamEnd {
  Complete,
  Interrupted { tools: ToolExecutionState },
  Failed(crate::Error),
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum IncompleteReason {
  OutputLimit,
  Interrupted { tools: ToolExecutionState },
  Failed(crate::Error),
}

/// Protocol-owned result of closing a model stream.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum StreamFinalization {
  Complete(Box<super::Response>),
  Incomplete(Box<PartialResponse>),
}

/// Incomplete output and a fully paired context fragment. Usage is observed, never estimated.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct PartialResponse {
  pub reason: IncompleteReason,
  pub blocks: Vec<PartialBlock>,
  pub usage: Usage,
  pub account_state: Option<AccountState>,
  pub upstream_stop: Option<StopReason>,
  /// Ready to append: includes protocol-generated results for retained tool calls.
  /// Empty on stream failure; interruption and output limits preserve eligible content.
  messages: Vec<Message>,
}

impl PartialResponse {
  /// Buffered replies carry no per-block completions. Conservatively discard tool
  /// calls and opaque reasoning at an output limit, while preserving text and transparent thought.
  pub fn from_output_limit(
    response: super::Response,
    protocol: Option<ModelUseProtocol>,
  ) -> Result<Self, crate::Error> {
    if response.stop_reason != StopReason::MaxOutputLengthExceeded {
      return Err(crate::Error::Build("expected an output-limited response".into()));
    }
    let rules = protocol.map(ModelUseProtocol::get_reasoning_replay).unwrap_or_default();
    let blocks: Vec<_> = response
      .messages
      .into_iter()
      .enumerate()
      .map(|(index, message)| {
        let replay = match &message {
          Message::Assistant { .. } => ReplayDisposition::Replayable,
          Message::Reasoning { plaintext, signature, ciphertext, .. } => {
            rules.classify(false, plaintext, signature, ciphertext)
          }
          _ => ReplayDisposition::Incomplete,
        };
        PartialBlock {
          index: index as u32,
          closed: false,
          content: PartialContent::Message(message),
          replay,
        }
      })
      .collect();
    let messages = build_context_fragment(&blocks, ToolExecutionState::NotStarted);
    Ok(Self {
      reason: IncompleteReason::OutputLimit,
      blocks,
      messages,
      usage: response.usage,
      account_state: response.account_state,
      upstream_stop: Some(response.stop_reason),
    })
  }

  /// Add output retained by an enclosing executor before this interrupted request began.
  pub(crate) fn prepend_replay_messages(&mut self, mut prefix: Vec<Message>) {
    prefix.append(&mut self.messages);
    self.messages = prefix;
  }

  /// Text/reasoning projection for automatic continuation. Tool invocations from a capped
  /// response are not executable and must be reissued; their paired signature is omitted too.
  pub fn get_continuation_messages(&self) -> Vec<Message> {
    self
      .messages
      .iter()
      .enumerate()
      .filter_map(|(index, message)| match message {
        Message::Assistant { .. } => Some(message.clone()),
        Message::Reasoning { plaintext, signature, .. } => {
          if plaintext.is_empty()
            && !signature.is_empty()
            && matches!(self.messages.get(index + 1), Some(Message::ToolUse { .. }))
          {
            None
          } else {
            Some(message.clone())
          }
        }
        _ => None,
      })
      .collect()
  }

  /// A complete context fragment, including tool-result pairing. Never execute its tool calls.
  pub fn get_replay_messages(&self) -> &[Message] {
    &self.messages
  }
}

fn build_context_fragment(blocks: &[PartialBlock], tools: ToolExecutionState) -> Vec<Message> {
  let mut messages = Vec::new();
  let mut results = Vec::new();
  for block in blocks {
    if block.replay != ReplayDisposition::Replayable {
      continue;
    }
    match &block.content {
      PartialContent::Message(message) => messages.push(message.clone()),
      PartialContent::ToolCall { call_id: Some(call_id), name: Some(name), arguments } => {
        let Some(arguments) = parse_arguments(arguments) else {
          continue;
        };
        messages.push(Message::ToolUse {
          metadata: Default::default(),
          call_id: call_id.clone(),
          name: name.clone(),
          arguments,
        });
        let content = match tools {
          ToolExecutionState::NotStarted => serde_json::json!({"status": "cancelled"}),
          ToolExecutionState::MayHaveStarted => serde_json::json!({
            "status": "unknown", "message": "Execution may have started; its outcome is unavailable."
          }),
        };
        results.push(Message::ToolResult {
          metadata: Default::default(),
          call_id: call_id.clone(),
          name: name.clone(),
          content,
        });
      }
      PartialContent::ToolCall { .. } => {}
    }
  }
  messages.extend(results);
  messages
}

/// A retained call's arguments, when they are a JSON object. Valid JSON is not completion: the
/// block's explicit completion is still required by the caller of this helper.
fn parse_arguments(arguments: &str) -> Option<Value> {
  parse_tool_arguments(arguments).ok().filter(Value::is_object)
}

impl StreamAccumulator {
  /// Normal completion stays strict. Explicit interruption preserves eligible content and pairs
  /// tools using execution facts; failed reading retains diagnostics but accepts no messages.
  pub fn finalize(self, end: StreamEnd) -> Result<StreamFinalization, crate::Error> {
    match end {
      StreamEnd::Complete => {
        self.finish().map(|response| StreamFinalization::Complete(Box::new(response)))
      }
      StreamEnd::Interrupted { tools } => {
        Ok(StreamFinalization::Incomplete(Box::new(self.interrupt(tools))))
      }
      StreamEnd::Failed(error) => Ok(StreamFinalization::Incomplete(Box::new(
        self.build_partial_response(IncompleteReason::Failed(error)),
      ))),
    }
  }

  /// Finalize an explicit interruption without flushing or inventing terminal events.
  /// The caller supplies execution facts. The returned context fragment is already tool-paired.
  pub fn interrupt(self, tools: ToolExecutionState) -> PartialResponse {
    self.build_partial_response(IncompleteReason::Interrupted { tools })
  }

  /// Finalize an upstream output limit using the same block-level replay rules as interruption.
  pub fn finish_output_limit(self) -> Result<PartialResponse, crate::Error> {
    if self.stop_reason != Some(StopReason::MaxOutputLengthExceeded) {
      return Err(crate::Error::Build(
        "output-limit finalization requires an upstream output limit".into(),
      ));
    }
    Ok(self.build_partial_response(IncompleteReason::OutputLimit))
  }

  fn build_partial_response(self, reason: IncompleteReason) -> PartialResponse {
    let rules = self.replay;
    let mut blocks = BTreeMap::new();
    let mut call_ids = HashSet::new();
    for (index, block) in self.blocks {
      let closed = block.closed;
      let (content, replay) = classify_block(block, &rules, &mut call_ids);
      blocks.insert(index, PartialBlock { index, closed, content, replay });
    }
    for (index, message) in self.items {
      blocks.insert(
        index,
        PartialBlock {
          index,
          closed: true,
          content: PartialContent::Message(message),
          replay: ReplayDisposition::Replayable,
        },
      );
    }
    let mut blocks: Vec<_> = blocks.into_values().collect();
    if rules.seals_next_part {
      mark_unsealed_signatures(&mut blocks);
    }
    let messages = match reason {
      IncompleteReason::Interrupted { tools } => build_context_fragment(&blocks, tools),
      IncompleteReason::OutputLimit => {
        build_context_fragment(&blocks, ToolExecutionState::NotStarted)
      }
      IncompleteReason::Failed(_) => Vec::new(),
    };
    PartialResponse {
      reason,
      messages,
      blocks,
      usage: self.usage.unwrap_or_default(),
      account_state: self.account_state,
      upstream_stop: self.stop_reason,
    }
  }
}

/// One streamed block as partial content, and whether it can be replayed: text always, reasoning by
/// the protocol's `rules`, and a tool call only once complete, with a fresh non-empty call id (one
/// the `call_ids` seen so far do not have), a name and object arguments.
fn classify_block(
  block: Block,
  rules: &ReasoningReplay,
  call_ids: &mut HashSet<String>,
) -> (PartialContent, ReplayDisposition) {
  match block.kind {
    BlockKind::Text => (
      PartialContent::Message(Message::Assistant {
        metadata: Default::default(),
        content: vec![ContentBlock::Text { text: block.text }],
      }),
      ReplayDisposition::Replayable,
    ),
    BlockKind::Reasoning => {
      let replay =
        rules.classify(block.complete, &block.plaintext, &block.signature, &block.ciphertext);
      (PartialContent::Message(block.into_reasoning(rules)), replay)
    }
    BlockKind::ToolUse => {
      let valid =
        block.call_id.as_ref().is_some_and(|id| !id.is_empty() && call_ids.insert(id.clone()))
          && block.name.as_ref().is_some_and(|name| !name.is_empty())
          && parse_arguments(&block.arguments).is_some();
      let replay = if !block.complete {
        ReplayDisposition::Incomplete
      } else if valid {
        ReplayDisposition::Replayable
      } else {
        ReplayDisposition::InvalidToolCall
      };
      (
        PartialContent::ToolCall {
          call_id: block.call_id,
          name: block.name,
          arguments: block.arguments,
        },
        replay,
      )
    }
  }
}

/// Signature-only parts seal the immediately following replayable model part. Never let a dropped
/// part cause the signature to migrate to some later text or tool call: a signature whose part is
/// not right behind it is marked as missing it.
fn mark_unsealed_signatures(blocks: &mut [PartialBlock]) {
  for index in 0..blocks.len() {
    if matches!(&blocks[index].content, PartialContent::Message(Message::Reasoning { plaintext, signature, .. })
      if plaintext.is_empty() && !signature.is_empty())
    {
      let paired = blocks.get(index + 1).is_some_and(|next| {
        next.replay == ReplayDisposition::Replayable
          && matches!(
            next.content,
            PartialContent::ToolCall { .. } | PartialContent::Message(Message::Assistant { .. })
          )
      });
      if !paired {
        blocks[index].replay = ReplayDisposition::MissingSignedPart;
      }
    }
  }
}
