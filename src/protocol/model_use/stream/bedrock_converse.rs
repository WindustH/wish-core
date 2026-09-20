//! Bedrock `converse-stream` wire: AWS event-stream frames whose payloads carry one JSON event
//! each, keyed by the frame's `:event-type`.
//!
//! Conversions:
//! - Blocks arrive as `contentBlockStart` / `contentBlockDelta` / `contentBlockStop` triples,
//!   numbered by `contentBlockIndex`. A `toolUse` start opens its block with the call id and name
//!   announced immediately; every other start does not say what the block is (text and reasoning
//!   both start empty), so those blocks open lazily on their first delta, when the delta's shape
//!   (`text` vs `reasoningContent`) has named the kind.
//! - Deltas stream: `delta.text` into a text block, `delta.reasoningContent.text` into a reasoning
//!   block with `.signature` and `.redactedContent` as its proof and stand-in (the buffered
//!   decoder's convention: the bare strings), and `delta.toolUse.input` into the tool block's
//!   arguments. A delta naming a block that never opened is tolerated, matching the wire's own
//!   tolerance, but a delta for a block of another kind fails closed at the accumulator.
//! - `contentBlockStop` closes its block; a stop for a block that never opened emits nothing.
//! - `messageStop` carries the `stopReason` and `metadata` the `usage`; both are required, and
//!   whichever arrives second is the terminal: open blocks close in index order and the buffered
//!   decoder's mappings produce the usage report and the stop. `messageStart`, `messageDelta`
//!   (which some models use instead, redundantly), unknown events and exception frames
//!   (`__exception:<type>`, payload not guaranteed JSON) map to an upstream error.
//!
//! Trade-offs:
//! - A body that ends without both terminal halves is a protocol error, reported by `finish()`.

use std::collections::HashMap;

use serde_json::Value;

use crate::protocol::error::Error;
use crate::protocol::model_use::response::bedrock_converse as buffered;
use crate::protocol::{BlockKind, StopReason, StreamEvent};

/// Decodes one `converse-stream` body.
#[derive(Default)]
pub struct Decoder {
  next_index: u32,
  /// `contentBlockIndex` -> our block index.
  blocks: HashMap<u64, u32>,
  finish_reason: Option<String>,
  usage: Option<crate::protocol::Usage>,
  done: bool,
}

impl Decoder {
  pub fn new() -> Self {
    Self::default()
  }

  /// Feeds one frame: its `:event-type` (or `__exception:<type>`) and its JSON payload.
  pub fn feed(&mut self, event: Option<&str>, data: &str) -> Result<Vec<StreamEvent>, Error> {
    if self.done {
      return Err(Error::Malformed("event after the terminal converse event".to_owned()));
    }
    let Some(event) = event else {
      return Err(Error::Malformed("converse frame without an event type".to_owned()));
    };
    if let Some(exception) = event.strip_prefix("__exception:") {
      return Err(decode_exception_error(exception, data));
    }
    let value: Value = serde_json::from_str(data)
      .map_err(|error| Error::Malformed(format!("converse event is not JSON: {error}")))?;
    let mut out = Vec::new();
    match event {
      "messageStart" | "messageDelta" => {}
      "contentBlockStart" => self.handle_block_start(&value, &mut out),
      "contentBlockDelta" => self.handle_block_delta(&value, &mut out),
      "contentBlockStop" => self.handle_block_stop(&value, &mut out),
      "messageStop" => {
        if let Some(reason) = value.pointer("/messageStop/stopReason").and_then(Value::as_str) {
          self.finish_reason = Some(reason.to_owned());
        }
      }
      "metadata" => {
        if let Some(usage) = value.pointer("/metadata/usage").filter(|usage| !usage.is_null()) {
          let usage = buffered::parse_usage(Some(usage));
          self.usage = Some(usage);
          out.push(StreamEvent::Usage(usage));
        }
      }
      // Unknown and future event types are tolerated.
      _ => {}
    }
    if self.finish_reason.is_some() && self.usage.is_some() {
      let reason = self.finish_reason.take();
      out.extend(self.terminate(reason.as_deref()));
    }
    Ok(out)
  }

  /// The body ended: the wire's two terminal halves must both have arrived.
  pub fn finish(&mut self) -> Result<Vec<StreamEvent>, Error> {
    if self.done {
      return Ok(Vec::new());
    }
    Err(Error::Malformed("stream ended before messageStop and metadata".to_owned()))
  }

  fn handle_block_start(&mut self, value: &Value, out: &mut Vec<StreamEvent>) {
    let Some(index) = value.pointer("/contentBlockStart/contentBlockIndex").and_then(Value::as_u64)
    else {
      return;
    };
    if self.blocks.contains_key(&index) {
      return;
    }
    let Some(tool) = value.pointer("/contentBlockStart/start/toolUse") else {
      // A text or reasoning block: its kind is only named by its first delta, so the block
      // opens there.
      return;
    };
    let block_index = self.next_index;
    self.next_index += 1;
    self.blocks.insert(index, block_index);
    out.push(StreamEvent::BlockStart { index: block_index, kind: BlockKind::ToolUse });
    out.push(StreamEvent::ToolUseDelta {
      index: block_index,
      call_id: Some(tool.get("toolUseId").and_then(Value::as_str).unwrap_or("").to_owned()),
      name: tool
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_owned),
      arguments: String::new(),
    });
  }

  fn handle_block_delta(&mut self, value: &Value, out: &mut Vec<StreamEvent>) {
    let Some(index) = value.pointer("/contentBlockDelta/contentBlockIndex").and_then(Value::as_u64)
    else {
      return;
    };
    let Some(delta) = value.pointer("/contentBlockDelta/delta") else { return };
    if let Some(text) = delta.get("text").and_then(Value::as_str) {
      let block = self.ensure(index, BlockKind::Text, out);
      out.push(StreamEvent::TextDelta { index: block, delta: text.to_owned() });
      return;
    }
    if let Some(reasoning) = delta.get("reasoningContent") {
      let block = self.ensure(index, BlockKind::Reasoning, out);
      if let Some(text) = reasoning.get("text").and_then(Value::as_str) {
        out.push(StreamEvent::ReasoningDelta { index: block, delta: text.to_owned() });
      }
      if let Some(signature) = reasoning.get("signature").and_then(Value::as_str) {
        out.push(StreamEvent::ReasoningSignatureDelta {
          index: block,
          signature: signature.to_owned(),
        });
      }
      if let Some(ciphertext) = reasoning.get("redactedContent").and_then(Value::as_str) {
        out.push(StreamEvent::ReasoningCiphertextDelta {
          index: block,
          ciphertext: ciphertext.to_owned(),
        });
      }
      return;
    }
    if let Some(input) = delta.pointer("/toolUse/input").and_then(Value::as_str)
      && let Some(block) = self.blocks.get(&index)
    {
      out.push(StreamEvent::ToolUseDelta {
        index: *block,
        call_id: None,
        name: None,
        arguments: input.to_owned(),
      });
    }
  }

  fn handle_block_stop(&mut self, value: &Value, out: &mut Vec<StreamEvent>) {
    let Some(index) = value.pointer("/contentBlockStop/contentBlockIndex").and_then(Value::as_u64)
    else {
      return;
    };
    if let Some(block) = self.blocks.remove(&index) {
      out.push(StreamEvent::BlockEnd { index: block });
    }
  }

  /// The block of a delta: opened get_current_time (with the kind the delta just named) if this is its first.
  fn ensure(&mut self, index: u64, kind: BlockKind, out: &mut Vec<StreamEvent>) -> u32 {
    if let Some(block) = self.blocks.get(&index) {
      return *block;
    }
    let block = self.next_index;
    self.next_index += 1;
    self.blocks.insert(index, block);
    out.push(StreamEvent::BlockStart { index: block, kind });
    block
  }

  /// Close every still-open block in index order, then stop.
  fn terminate(&mut self, reason: Option<&str>) -> Vec<StreamEvent> {
    self.done = true;
    let mut indices: Vec<u32> = self.blocks.values().copied().collect();
    indices.sort_unstable();
    indices.dedup();
    let mut out: Vec<StreamEvent> =
      indices.into_iter().map(|index| StreamEvent::BlockEnd { index }).collect();
    let stop = match reason {
      Some(reason) => buffered::map_stop_reason(Some(reason)),
      None => StopReason::Unknown,
    };
    out.push(StreamEvent::Stop(stop));
    out
  }
}

/// Maps an exception frame: the type is the code, the payload is the message when it is not JSON.
fn decode_exception_error(exception: &str, data: &str) -> Error {
  let message = serde_json::from_str::<Value>(data)
    .ok()
    .and_then(|value| value.get("message").and_then(Value::as_str).map(str::to_owned))
    .unwrap_or_else(|| data.to_owned());
  Error::from_in_band(Some(exception.to_owned()), message)
}
