//! Interrupted output and its conservative projection into replayable protocol messages.
use super::{BlockKind, ContentBlock, Message, StopReason, StreamAccumulator, Usage};
use crate::protocol::{account_state::AccountState, model_use::ModelUseProtocol};
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
  /// Empty on stream failure; explicit interruption preserves eligible content.
  messages: Vec<Message>,
}

impl PartialResponse {
  /// A complete context fragment, including tool-result pairing. Never execute its tool calls.
  pub fn get_replay_messages(&self) -> &[Message] {
    &self.messages
  }

  /// Transfer the context fragment while keeping the original partial data for inspection.
  pub fn take_replay_messages(&mut self) -> Vec<Message> {
    std::mem::take(&mut self.messages)
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

fn parse_arguments(arguments: &str) -> Option<Value> {
  // An explicit block completion is still required by the caller of this helper.
  if arguments.trim().is_empty() {
    Some(serde_json::json!({}))
  } else {
    serde_json::from_str::<Value>(arguments).ok().filter(Value::is_object)
  }
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

  fn build_partial_response(self, reason: IncompleteReason) -> PartialResponse {
    let mut blocks = BTreeMap::new();
    let mut ids = HashSet::new();
    for (index, block) in self.blocks {
      let (content, replay) = match block.kind {
        BlockKind::Text => (
          PartialContent::Message(Message::Assistant {
            metadata: Default::default(),
            content: vec![ContentBlock::Text { text: block.text }],
          }),
          ReplayDisposition::Replayable,
        ),
        BlockKind::Reasoning => {
          let replay = classify_reasoning_replay(
            self.protocol,
            block.complete,
            &block.plaintext,
            &block.signature,
            &block.ciphertext,
          );
          (
            PartialContent::Message(Message::Reasoning {
              metadata: Default::default(),
              display: block.display.unwrap_or_else(|| block.plaintext.clone()),
              plaintext: block.plaintext,
              signature: block.signature,
              ciphertext: block.ciphertext,
            }),
            replay,
          )
        }
        BlockKind::ToolUse => {
          let valid =
            block.call_id.as_ref().is_some_and(|id| !id.is_empty() && ids.insert(id.clone()))
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
      };
      blocks.insert(index, PartialBlock { index, closed: block.closed, content, replay });
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
    if matches!(
      self.protocol,
      Some(ModelUseProtocol::GoogleGenerateContent | ModelUseProtocol::GoogleVertexGenerateContent)
    ) {
      // Signature-only parts seal the immediately following replayable model part. Never let a
      // dropped part cause the signature to migrate to some later text or tool call.
      for index in 0..blocks.len() {
        if matches!(&blocks[index].content, PartialContent::Message(Message::Reasoning { plaintext, signature, .. })
          if plaintext.is_empty() && !signature.is_empty())
        {
          let paired = blocks.get(index + 1).is_some_and(|next| {
            next.replay == ReplayDisposition::Replayable
              && matches!(
                next.content,
                PartialContent::ToolCall { .. }
                  | PartialContent::Message(Message::Assistant { .. })
              )
          });
          if !paired {
            blocks[index].replay = ReplayDisposition::MissingSignedPart;
          }
        }
      }
    }
    let messages = match reason {
      IncompleteReason::Interrupted { tools } => build_context_fragment(&blocks, tools),
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

fn classify_reasoning_replay(
  protocol: Option<ModelUseProtocol>,
  complete: bool,
  plaintext: &str,
  signature: &str,
  ciphertext: &str,
) -> ReplayDisposition {
  use crate::protocol::model_use::request::{
    anthropic_messages::MessagesApiCompatMode, openai_chat::ChatCompletionApiCompatMode,
    openai_responses::ReasoningForm,
  };
  // Transparency is determined by the wire's replay representation, not by visible text or
  // by a signature/ciphertext that has not arrived yet. Display summaries are separate data.
  let transparent = match protocol {
    Some(ModelUseProtocol::OpenAiResponses(mode)) => {
      mode.reasoning_form == ReasoningForm::Plaintext
    }
    Some(ModelUseProtocol::OpenAiChat(mode)) => mode != ChatCompletionApiCompatMode::Official,
    Some(ModelUseProtocol::AnthropicMessages(mode)) => mode != MessagesApiCompatMode::Official,
    Some(ModelUseProtocol::MistralConversations) => true,
    _ => false,
  };
  if transparent && signature.is_empty() && ciphertext.is_empty() && !plaintext.is_empty() {
    return ReplayDisposition::Replayable;
  }
  if !complete {
    return ReplayDisposition::Incomplete;
  }
  let replayable = match protocol {
    Some(ModelUseProtocol::OpenAiResponses(mode)) => match mode.reasoning_form {
      ReasoningForm::Plaintext => !plaintext.is_empty(),
      ReasoningForm::Ciphertext => !ciphertext.is_empty(),
      ReasoningForm::NoSendBack => false,
    },
    // An opaque state can arrive without visible text; the text is not completeness evidence.
    Some(ModelUseProtocol::AnthropicMessages(_) | ModelUseProtocol::BedrockConverse) => {
      !ciphertext.is_empty() || !signature.is_empty()
    }
    Some(
      ModelUseProtocol::GoogleGenerateContent
      | ModelUseProtocol::GoogleVertexGenerateContent
      | ModelUseProtocol::GoogleInteractions,
    ) => !signature.is_empty(),
    Some(ModelUseProtocol::OpenAiChat(_) | ModelUseProtocol::MistralConversations) => {
      transparent && !plaintext.is_empty()
    }
    None => false,
  };
  if replayable { ReplayDisposition::Replayable } else { ReplayDisposition::NotReplayable }
}
