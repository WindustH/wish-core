//! OpenAI Chat Completions stream wire.
//!
//! Conversions:
//! - This wire has no block events, so the decoder numbers blocks itself, lazily, in the order
//!   kinds first appear: a text block opens with the first content delta, one reasoning block with
//!   the first thought, and one tool block per upstream `tool_calls` index. All of them close when
//!   `[DONE]` arrives, in index order.
//! - `finish_reason` is remembered; `[DONE]` closes the blocks and emits `Stop`. A `[DONE]` without
//!   a finish reason is a protocol error, and a body that ends without `[DONE]` leaves the
//!   accumulator without its stop event.
//! - The usage chunk (only sent because the request wire asks for `stream_options.include_usage`)
//!   and the finish reason map through the buffered decoder's own mappings.
//! - A delta under the mode's reasoning field opens a reasoning block, and so does a `thinking`
//!   chunk in a `content` list (Mistral's spelling); the block closes with the rest, in index order.
//! - MiniMax's refusal (`base_resp.status_code`) ends the stream with the same in-band error the
//!   buffered decoder reports for it.
//!
//! Trade-offs:
//! - Only `choices[0]` is read, like the buffered decoder.
//! - A tool fragment without an index routes to the only open tool block and fails closed when it
//!   is ambiguous.

use std::collections::HashMap;

use serde_json::Value;

use super::WireDecoder;
use super::blocks::{LazyBlocks, end_blocks_in_order};
use crate::protocol::error::Error;
use crate::protocol::http_error::decode_in_band;
use crate::protocol::model_use::mode::{ChatCompletionApiCompatMode, REASONING_FIELD};
use crate::protocol::model_use::response::openai_chat as buffered;
use crate::protocol::{BlockKind, StreamEvent};

/// Decodes one chat completions stream.
#[derive(Default)]
pub struct Decoder {
  mode: ChatCompletionApiCompatMode,
  blocks: LazyBlocks,
  /// Upstream `tool_calls` index -> our block index.
  tool_blocks: HashMap<u32, u32>,
  finish_reason: Option<String>,
  done: bool,
}

impl Decoder {
  pub fn new(mode: ChatCompletionApiCompatMode) -> Self {
    Self { mode, ..Self::default() }
  }

  /// `[DONE]`: close every open block in index order, then stop.
  fn terminate(&mut self) -> Result<Vec<StreamEvent>, Error> {
    self.done = true;
    let finish = self
      .finish_reason
      .as_deref()
      .ok_or_else(|| Error::Malformed("stream ended without a finish_reason".to_owned()))?;
    let mut out = Vec::new();
    let indices = self.tool_blocks.values().copied().chain(self.blocks.get_opened()).collect();
    end_blocks_in_order(indices, &mut out);
    out.push(StreamEvent::Stop(buffered::map_stop_reason(Some(finish), self.mode)));
    Ok(out)
  }

  fn decode_tool_fragment(
    &mut self,
    fragment: &Value,
    out: &mut Vec<StreamEvent>,
  ) -> Result<(), Error> {
    let upstream = match fragment.get("index").and_then(Value::as_u64) {
      Some(index) => u32::try_from(index).map_err(|_| {
        Error::Malformed(format!("tool_calls delta index {index} does not fit u32"))
      })?,
      None if self.tool_blocks.len() == 1 => *self.tool_blocks.keys().next().expect("one block"),
      None => {
        return Err(Error::Malformed(
          "tool_calls delta without an index while it is ambiguous".to_owned(),
        ));
      }
    };
    let index = match self.tool_blocks.get(&upstream) {
      Some(index) => *index,
      None => {
        let index = self.blocks.counter.open(BlockKind::ToolUse, out);
        self.tool_blocks.insert(upstream, index);
        index
      }
    };
    let function = fragment.get("function");
    let field =
      |name: &str| function.and_then(|function| function.get(name)).and_then(Value::as_str);
    out.push(StreamEvent::ToolUseDelta {
      index,
      call_id: fragment
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned),
      name: field("name").filter(|name| !name.is_empty()).map(str::to_owned),
      arguments: field("arguments").unwrap_or_default().to_owned(),
    });
    Ok(())
  }
}

impl WireDecoder for Decoder {
  /// Feeds one SSE record. The `event:` name is always absent on this wire.
  fn feed(&mut self, _event: Option<&str>, data: &str) -> Result<Vec<StreamEvent>, Error> {
    if self.done {
      return Err(Error::Malformed("chunk after [DONE]".to_owned()));
    }
    if data.trim() == "[DONE]" {
      return self.terminate();
    }
    let payload: Value = serde_json::from_str(data)
      .map_err(|error| Error::Malformed(format!("chat chunk is not JSON: {error}")))?;
    let mut out = Vec::new();

    // OpenAI-compatible servers may push a terminal error object mid-stream.
    if let Some(error) = payload.get("error").filter(|error| error.is_object()) {
      return Err(decode_in_band(
        Some(error),
        &["code", "type"],
        "upstream reported an error without a message",
      ));
    }
    if payload.get("usage").is_some_and(|usage| !usage.is_null()) {
      out.push(StreamEvent::Usage(buffered::decode_usage(&payload)));
    }
    // The same refusal the buffered decoder reads, arriving as a chunk.
    if self.mode == ChatCompletionApiCompatMode::MiniMax
      && let Some(error) = buffered::decode_refusal_error(&payload)
    {
      return Err(error);
    }
    let Some(choice) = payload.pointer("/choices/0") else { return Ok(out) };
    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
      self.finish_reason = Some(reason.to_owned());
    }
    let Some(delta) = choice.get("delta").filter(|delta| delta.is_object()) else {
      return Ok(out);
    };
    if let Some(content) = delta.get("content").filter(|content| !content.is_null()) {
      match content {
        Value::String(text) => self.blocks.push_text(text, &mut out),
        Value::Array(chunks) if self.mode.is_mistral() => {
          self.blocks.push_chunks(chunks, &mut out)?
        }
        Value::Array(_) => {
          return Err(Error::Malformed(
            "`content` is a chunk list, which this variant does not read".to_owned(),
          ));
        }
        _ => return Err(Error::Malformed("`content` delta is not a string".to_owned())),
      }
    }
    // Only the flat `reasoning_content` delta carries reasoning here: the official wire has no such
    // field, and Mistral spells its reasoning in `content` chunks instead.
    if self.mode.has_reasoning_field() {
      let name = REASONING_FIELD;
      if let Some(value) = delta.get(name).filter(|value| !value.is_null()) {
        let text = value
          .as_str()
          .ok_or_else(|| Error::Malformed(format!("`{name}` delta is not a string")))?;
        self.blocks.push_reasoning(text, &mut out);
      }
    }
    if let Some(fragments) = delta.get("tool_calls").and_then(Value::as_array) {
      for fragment in fragments {
        self.decode_tool_fragment(fragment, &mut out)?;
      }
    }
    Ok(out)
  }
}
