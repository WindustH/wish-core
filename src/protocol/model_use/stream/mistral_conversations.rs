//! Mistral Conversations stream wire.
//!
//! Conversions:
//! - The events are typed: `message.output.delta` carries text or a chunk list,
//!   `function.call.delta` a tool call, `conversation.response.done` the usage and the end of the
//!   turn, `conversation.response.error` a failure reported mid-stream.
//! - Like the chat wire, this one has no block events, so blocks are numbered lazily in the order
//!   their kind first appears, and all of them close at the end of the turn, in index order.
//! - A tool call is one block per `tool_call_id`: the first delta opens it and the fragments that
//!   follow append to its arguments.
//!
//! Trade-offs:
//! - The stop reason is the buffered decoder's reading rather than the wire's: a tool block means
//!   the turn stopped to have a tool run, and anything else is `Stop`.
//! - `tool.execution.*` and `agent.handoff.*` are dropped: they belong to connectors and agents this
//!   crate does not model, and `handoff_execution: "client"` keeps them away in the first place. A
//!   `[DONE]` sentinel is dropped too, if this wire turns out to send one.
//! - The vendor's prose shortens the event names (`response.done` for what its schema spells
//!   `conversation.response.done`), so both spellings are read, from the payload's own `type` member
//!   first and from the SSE `event` name after it.
//! - A delta whose `arguments` repeated the whole call instead of a fragment would be appended
//!   twice; the sibling `tool.execution` events call theirs incremental, so fragments are assumed.
//! - A body that ends without a `conversation.response.done` produces no terminal event: the
//!   accumulator then reports the missing stop, rather than this decoder inventing one.

use std::collections::HashMap;

use serde_json::Value;

use crate::protocol::error::Error;
use crate::protocol::model_use::response::mistral_conversations as buffered;
use crate::protocol::{BlockKind, StopReason, StreamEvent};

/// Decodes one conversations stream.
#[derive(Default)]
pub struct Decoder {
  next_index: u32,
  text_block: Option<u32>,
  reasoning_block: Option<u32>,
  tool_blocks: HashMap<String, u32>,
  saw_tool_use: bool,
  done: bool,
}

impl Decoder {
  pub fn new() -> Self {
    Self::default()
  }

  /// Feeds one SSE record: the payload's own `type`, or the event name when it carries none.
  pub fn feed(&mut self, event: Option<&str>, data: &str) -> Result<Vec<StreamEvent>, Error> {
    if data.trim() == "[DONE]" {
      return Ok(Vec::new());
    }
    if self.done {
      return Err(Error::Malformed("event after the conversation ended".to_owned()));
    }
    let payload: Value = serde_json::from_str(data)
      .map_err(|error| Error::Malformed(format!("conversation event is not JSON: {error}")))?;
    let kind = payload.get("type").and_then(Value::as_str).or(event).unwrap_or("");
    let mut out = Vec::new();
    match kind {
      "conversation.response.started" | "response.started" => {}
      "message.output.delta" | "message.output" => {
        if let Some(content) = payload.get("content").filter(|content| !content.is_null()) {
          self.decode_content(content, &mut out)?;
        }
      }
      "function.call.delta" | "function.call" => self.decode_function_call(&payload, &mut out)?,
      "conversation.response.done" | "response.done" => {
        if payload.get("usage").is_some_and(|usage| !usage.is_null()) {
          out.push(StreamEvent::Usage(buffered::parse_usage(&payload)));
        }
        out.extend(self.finish());
      }
      "conversation.response.error" | "response.error" => {
        let code = payload.get("code").map(|code| match code {
          Value::String(code) => code.clone(),
          other => other.to_string(),
        });
        return Err(Error::from_in_band(
          code,
          payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("upstream reported an error without a message")
            .to_owned(),
        ));
      }
      _ => {}
    }
    Ok(out)
  }

  /// Closes every open block in index order, then stops the turn.
  fn finish(&mut self) -> Vec<StreamEvent> {
    self.done = true;
    let mut indices: Vec<u32> = self.tool_blocks.values().copied().collect();
    indices.extend(self.text_block);
    indices.extend(self.reasoning_block);
    indices.sort_unstable();
    let stop = if self.saw_tool_use { StopReason::ToolUse } else { StopReason::Stop };
    indices
      .into_iter()
      .map(|index| StreamEvent::BlockEnd { index })
      .chain([StreamEvent::Stop(stop)])
      .collect()
  }

  /// One `content` delta: a plain string, or a chunk list carrying text and thoughts.
  fn decode_content(&mut self, content: &Value, out: &mut Vec<StreamEvent>) -> Result<(), Error> {
    match content {
      Value::String(text) => self.emit_text_delta(text, out),
      Value::Array(chunks) => {
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
            let Some(text) =
              part.get("text").and_then(Value::as_str).filter(|text| !text.is_empty())
            else {
              continue;
            };
            let index = self.open_reasoning(out);
            out.push(StreamEvent::ReasoningDelta { index, delta: text.to_owned() });
          }
        }
      }
      _ => {
        return Err(Error::Malformed(
          "`content` delta is neither a string nor a chunk list".to_owned(),
        ));
      }
    }
    Ok(())
  }

  /// One `function.call.delta`: one block per `tool_call_id`, opened by its first fragment.
  fn decode_function_call(
    &mut self,
    payload: &Value,
    out: &mut Vec<StreamEvent>,
  ) -> Result<(), Error> {
    let call_id = payload.get("tool_call_id").and_then(Value::as_str).ok_or_else(|| {
      Error::Malformed("function.call delta is missing `tool_call_id`".to_owned())
    })?;
    let index = match self.tool_blocks.get(call_id) {
      Some(index) => *index,
      None => {
        let index = self.next_index;
        self.next_index += 1;
        self.tool_blocks.insert(call_id.to_owned(), index);
        self.saw_tool_use = true;
        out.push(StreamEvent::BlockStart { index, kind: BlockKind::ToolUse });
        index
      }
    };
    out.push(StreamEvent::ToolUseDelta {
      index,
      call_id: Some(call_id.to_owned()),
      name: payload
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .map(str::to_owned),
      arguments: payload.get("arguments").and_then(Value::as_str).unwrap_or_default().to_owned(),
    });
    Ok(())
  }

  /// One text delta, ignoring the empty ones a chunk list is full of.
  fn emit_text_delta(&mut self, text: &str, out: &mut Vec<StreamEvent>) {
    if text.is_empty() {
      return;
    }
    let index = self.open_text(out);
    out.push(StreamEvent::TextDelta { index, delta: text.to_owned() });
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
}
