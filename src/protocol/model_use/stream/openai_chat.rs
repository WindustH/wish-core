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
//! - A delta under the variant's reasoning field opens a reasoning block, and so does a `thinking`
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

use crate::protocol::error::Error;
use crate::protocol::model_use::request::openai_chat::{
  ChatCompletionApiCompatMode, REASONING_FIELD,
};
use crate::protocol::model_use::response::openai_chat as buffered;
use crate::protocol::{BlockKind, StreamEvent};

/// Decodes one chat completions stream.
#[derive(Default)]
pub struct Decoder {
  mode: ChatCompletionApiCompatMode,
  next_index: u32,
  text_block: Option<u32>,
  reasoning_block: Option<u32>,
  tool_blocks: HashMap<u32, u32>,
  finish_reason: Option<String>,
  done: bool,
}

impl Decoder {
  pub fn new(mode: ChatCompletionApiCompatMode) -> Self {
    Self { mode, ..Self::default() }
  }

  /// Feeds one SSE record. The `event:` name is always absent on this wire.
  pub fn feed(&mut self, _event: Option<&str>, data: &str) -> Result<Vec<StreamEvent>, Error> {
    if self.done {
      return Err(Error::Malformed("chunk after [DONE]".to_owned()));
    }
    if data.trim() == "[DONE]" {
      return self.finish();
    }
    let payload: Value = serde_json::from_str(data)
      .map_err(|error| Error::Malformed(format!("chat chunk is not JSON: {error}")))?;
    let mut out = Vec::new();

    // OpenAI-compatible servers may push a terminal error object mid-stream.
    if let Some(error) = payload.get("error").filter(|error| error.is_object()) {
      return Err(Error::from_in_band(
        error.get("code").or_else(|| error.get("type")).and_then(Value::as_str).map(str::to_owned),
        error
          .get("message")
          .and_then(Value::as_str)
          .unwrap_or("upstream reported an error without a message")
          .to_owned(),
      ));
    }
    if payload.get("usage").is_some_and(|usage| !usage.is_null()) {
      out.push(StreamEvent::Usage(buffered::parse_usage(&payload)));
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
        Value::String(text) => self.emit_text_delta(text, &mut out),
        Value::Array(chunks) if self.mode.is_mistral() => {
          self.decode_chunk_deltas(chunks, &mut out)?
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
    if !self.mode.is_plain() && !self.mode.is_mistral() {
      let name = REASONING_FIELD;
      if let Some(value) = delta.get(name).filter(|value| !value.is_null()) {
        let text = value
          .as_str()
          .ok_or_else(|| Error::Malformed(format!("`{name}` delta is not a string")))?;
        if !text.is_empty() {
          let index = self.open_reasoning(&mut out);
          out.push(StreamEvent::ReasoningDelta { index, delta: text.to_owned() });
        }
      }
    }
    if let Some(fragments) = delta.get("tool_calls").and_then(Value::as_array) {
      for fragment in fragments {
        self.decode_tool_fragment(fragment, &mut out)?;
      }
    }
    Ok(out)
  }

  /// `[DONE]`: close every open block in index order, then stop.
  fn finish(&mut self) -> Result<Vec<StreamEvent>, Error> {
    self.done = true;
    let finish = self
      .finish_reason
      .as_deref()
      .ok_or_else(|| Error::Malformed("stream ended without a finish_reason".to_owned()))?;
    let mut indices: Vec<u32> = self.tool_blocks.values().copied().collect();
    indices.extend(self.text_block);
    indices.extend(self.reasoning_block);
    indices.sort_unstable();
    let out: Vec<StreamEvent> = indices
      .into_iter()
      .map(|index| StreamEvent::BlockEnd { index })
      .chain([StreamEvent::Stop(buffered::map_stop_reason(Some(finish), self.mode))])
      .collect();
    Ok(out)
  }

  /// One text delta, ignoring the empty ones a chunk list is full of.
  fn emit_text_delta(&mut self, text: &str, out: &mut Vec<StreamEvent>) {
    if text.is_empty() {
      return;
    }
    let index = self.open_text(out);
    out.push(StreamEvent::TextDelta { index, delta: text.to_owned() });
  }

  /// One `content` chunk list: text chunks stream text, thought chunks stream reasoning.
  fn decode_chunk_deltas(
    &mut self,
    chunks: &[Value],
    out: &mut Vec<StreamEvent>,
  ) -> Result<(), Error> {
    for chunk in chunks {
      if buffered::get_chunk_type(chunk) == Some("text") {
        if let Some(text) = chunk.get("text").and_then(Value::as_str) {
          self.emit_text_delta(text, out);
        }
        continue;
      }
      if !buffered::is_thinking(chunk) {
        continue;
      }
      let parts = chunk.get("thinking").and_then(Value::as_array).ok_or_else(|| {
        Error::Malformed("`thinking` chunk carries no `thinking` list".to_owned())
      })?;
      for part in parts {
        let Some(text) = part.get("text").and_then(Value::as_str).filter(|text| !text.is_empty())
        else {
          continue;
        };
        let index = self.open_reasoning(out);
        out.push(StreamEvent::ReasoningDelta { index, delta: text.to_owned() });
      }
    }
    Ok(())
  }

  fn open_text(&mut self, out: &mut Vec<StreamEvent>) -> u32 {
    if let Some(index) = self.text_block {
      return index;
    }
    let index = self.next_index;
    self.next_index += 1;
    self.text_block = Some(index);
    out.push(StreamEvent::BlockStart { index, kind: BlockKind::Text });
    index
  }

  fn open_reasoning(&mut self, out: &mut Vec<StreamEvent>) -> u32 {
    if let Some(index) = self.reasoning_block {
      return index;
    }
    let index = self.next_index;
    self.next_index += 1;
    self.reasoning_block = Some(index);
    out.push(StreamEvent::BlockStart { index, kind: BlockKind::Reasoning });
    index
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
        let index = self.next_index;
        self.next_index += 1;
        self.tool_blocks.insert(upstream, index);
        out.push(StreamEvent::BlockStart { index, kind: BlockKind::ToolUse });
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
