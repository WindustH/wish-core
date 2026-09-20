//! OpenAI Responses stream wire.
//!
//! Conversions:
//! - This wire announces its own blocks: `output_item.added` opens a block for each `function_call`
//!   and `reasoning` item, and `content_part.added` opens the text block of a `message` item (first
//!   `output_text` part only; later parts of that item land in the same block). Indices are handed
//!   out in announcement order, and blocks close on their own `output_item.done`.
//! - Deltas attribute by `item_id`: `output_text.delta` streams text, `reasoning_text.delta` and
//!   `response.reasoning_summary_text.delta` both stream the reasoning text the accumulator mirrors
//!   into `display`, and `function_call_arguments.delta` streams argument JSON. A delta without an
//!   `item_id` cannot be attributed at all and fails closed.
//! - A delta that arrives before its item is announced is buffered (bounded) and flushed when the
//!   announcement arrives; bytes still orphaned at the terminal fail closed instead of dropping
//!   content silently.
//! - The reasoning item's `encrypted_content` (captured at `added` or `done`, whichever carries it
//!   first) is emitted once, as the block's ciphertext half.
//! - The item a compacted history travels as (`compaction`) is not streamed in parts: it is handed
//!   over whole at its `output_item.done`, as [`StreamEvent::UpstreamCompaction`], and takes the place its
//!   index gives it among the blocks.
//! - The terminal `response.completed` / `response.done` / `response.incomplete` /
//!   `response.cancelled` closes every open block in index order, reports the terminal usage, and
//!   stops, with the stop reason and usage mapped by the buffered decoder's own mappings (a
//!   completed stream with tool calls is `ToolUse`). `response.failed` is an upstream error, like
//!   the buffered decoder.
//! - The `response.rate_limits` frame is the account's own report riding the stream, and becomes a
//!   [`StreamEvent::Account`] instead of any block; `codex.rate_limits` is the same frame under the
//!   name the Codex documentation gives it.
//! - `response.created` / `response.in_progress` / `response.queued`, the standalone
//!   `response.usage` event, unknown event types and untyped payloads are all tolerated silently:
//!   none of them carry output.
//!
//! Trade-offs:
//! - Events after the terminal fail closed, and a body that ends without a terminal leaves the
//!   accumulator without its stop event.
//! - A rate-limit frame the account reader cannot make sense of is dropped rather than failing the
//!   answer: it is telemetry beside the answer, not part of it.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::protocol::account_state::codex;
use crate::protocol::error::Error;
use crate::protocol::model_use::response::openai_responses as buffered;
use crate::protocol::upstream_compaction::response::openai_responses as compaction;
use crate::protocol::{BlockKind, StreamEvent};

/// Bounded tolerance for deltas that arrive before their item is announced.
const MAX_ORPHAN_DELTA_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemShape {
  Text,
  Reasoning,
  ReasoningSummary,
  ToolUse,
}

impl ItemShape {
  fn get_kind(self) -> BlockKind {
    match self {
      ItemShape::Text => BlockKind::Text,
      ItemShape::Reasoning | ItemShape::ReasoningSummary => BlockKind::Reasoning,
      ItemShape::ToolUse => BlockKind::ToolUse,
    }
  }
}

/// Decodes one Responses stream.
#[derive(Default)]
pub struct Decoder {
  next_index: u32,
  /// `item_id` -> (block index, shape).
  blocks: HashMap<String, (u32, ItemShape)>,
  open: Vec<u32>,
  /// Deltas that arrived before their item was announced, per item, and their total size.
  orphans: HashMap<String, Vec<(ItemShape, String)>>,
  orphan_bytes: usize,
  /// Reasoning blocks whose ciphertext half was already emitted.
  ciphertext_seen: HashSet<u32>,
  done: bool,
}

impl Decoder {
  pub fn new() -> Self {
    Self::default()
  }

  /// Codex's quota rides the stream as a frame of its own: it says what the account has left
  /// without being part of the answer, so it becomes a reading rather than a block.
  fn decode_rate_limits(&self, value: &Value, out: &mut Vec<StreamEvent>) {
    let Some(rate_limits) = value.get("rate_limits") else { return };
    let payload = json!({ "rate_limits": rate_limits, "plan_type": value.get("plan_type") });
    if let Ok(snapshot) = codex::parse(&payload) {
      out.push(StreamEvent::Account(snapshot));
    }
  }

  /// Feeds one SSE record. The `event:` name is redundant with the payload's `type`.
  pub fn feed(&mut self, _event: Option<&str>, data: &str) -> Result<Vec<StreamEvent>, Error> {
    if self.done {
      return Err(Error::Malformed("event after the terminal response event".to_owned()));
    }
    let value: Value = serde_json::from_str(data)
      .map_err(|error| Error::Malformed(format!("responses event is not JSON: {error}")))?;
    let Some(event_type) = value.get("type").and_then(Value::as_str) else {
      return Ok(Vec::new());
    };
    let mut out = Vec::new();
    match event_type {
      "response.created" | "response.in_progress" | "response.queued" => {}
      "response.rate_limits" | "codex.rate_limits" => self.decode_rate_limits(&value, &mut out),
      "response.usage" => {}
      "response.output_item.added" => self.handle_item_added(&value, &mut out),
      "response.content_part.added" => self.handle_part_added(&value, &mut out),
      "response.output_text.delta" => self.handle_delta(&value, ItemShape::Text, &mut out)?,
      "response.reasoning_text.delta" => {
        self.handle_delta(&value, ItemShape::Reasoning, &mut out)?
      }
      "response.reasoning_summary_text.delta" => {
        self.handle_delta(&value, ItemShape::ReasoningSummary, &mut out)?
      }
      "response.function_call_arguments.delta" => {
        self.handle_delta(&value, ItemShape::ToolUse, &mut out)?
      }
      "response.output_item.done" => self.handle_item_done(&value, &mut out)?,
      "response.completed" | "response.done" | "response.incomplete" | "response.cancelled" => {
        return self.handle_terminal(&value);
      }
      "response.failed" | "response.error" | "error" => {
        let response = value.get("response").unwrap_or(&value);
        self.done = true;
        return Err(buffered::decode_upstream_error(response));
      }
      // Unknown and future event types are tolerated.
      _ => {}
    }
    Ok(out)
  }

  fn handle_item_added(&mut self, value: &Value, out: &mut Vec<StreamEvent>) {
    let Some(item) = value.get("item") else { return };
    let shape = match item.get("type").and_then(Value::as_str) {
      Some("function_call") => ItemShape::ToolUse,
      Some("reasoning") => ItemShape::Reasoning,
      // A message item gets its block from its first output_text content part.
      _ => return,
    };
    let item_id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    if item_id.is_empty() || self.blocks.contains_key(&item_id) {
      return;
    }
    let index = self.next_index;
    self.next_index += 1;
    self.blocks.insert(item_id.clone(), (index, shape));
    self.open.push(index);
    out.push(StreamEvent::BlockStart { index, kind: shape.get_kind() });
    match shape {
      ItemShape::ToolUse => {
        // The item announces the call id and name; arguments follow as deltas.
        let call_id = item
          .get("call_id")
          .and_then(Value::as_str)
          .filter(|call_id| !call_id.is_empty())
          .unwrap_or(&item_id);
        out.push(StreamEvent::ToolUseDelta {
          index,
          call_id: Some(call_id.to_owned()),
          name: item
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
          arguments: String::new(),
        });
      }
      ItemShape::Reasoning => self.emit_reasoning_ciphertext(index, item, out),
      ItemShape::Text | ItemShape::ReasoningSummary => {}
    }
    self.flush_orphans(&item_id, index, out);
  }

  fn handle_part_added(&mut self, value: &Value, out: &mut Vec<StreamEvent>) {
    let Some(part) = value.get("part") else { return };
    if part.get("type").and_then(Value::as_str) != Some("output_text") {
      return;
    }
    let Some(item_id) = value.get("item_id").and_then(Value::as_str) else { return };
    if self.blocks.contains_key(item_id) {
      return;
    }
    let index = self.next_index;
    self.next_index += 1;
    self.blocks.insert(item_id.to_owned(), (index, ItemShape::Text));
    self.open.push(index);
    out.push(StreamEvent::BlockStart { index, kind: BlockKind::Text });
    self.flush_orphans(item_id, index, out);
  }

  fn handle_delta(
    &mut self,
    value: &Value,
    shape: ItemShape,
    out: &mut Vec<StreamEvent>,
  ) -> Result<(), Error> {
    let Some(item_id) = value.get("item_id").and_then(Value::as_str) else {
      return Err(Error::Malformed(
        "delta event without an item_id cannot be attributed to a block".to_owned(),
      ));
    };
    let delta = value.get("delta").and_then(Value::as_str).unwrap_or_default().to_owned();
    let Some(&(index, _)) = self.blocks.get(item_id) else {
      self.orphan_bytes += delta.len();
      if self.orphan_bytes > MAX_ORPHAN_DELTA_BYTES {
        return Err(Error::Malformed(format!(
          "late deltas for unannounced items exceeded {MAX_ORPHAN_DELTA_BYTES} bytes"
        )));
      }
      self.orphans.entry(item_id.to_owned()).or_default().push((shape, delta));
      return Ok(());
    };
    self.emit_delta(index, shape, &delta, out);
    Ok(())
  }

  fn handle_item_done(&mut self, value: &Value, out: &mut Vec<StreamEvent>) -> Result<(), Error> {
    let Some(item) = value.get("item") else { return Ok(()) };
    // A compacted history arrives as one whole item rather than as blocks, so it takes an index of
    // its own among them and travels on as it came.
    if item.get("type").and_then(Value::as_str) == Some("compaction") {
      let index = self.next_index;
      self.next_index += 1;
      let (id, encrypted_content) = compaction::decode_compaction_payload(item)?;
      out.push(StreamEvent::UpstreamCompaction { index, id, encrypted_content });
      return Ok(());
    }
    let Some(item_id) = item.get("id").and_then(Value::as_str) else { return Ok(()) };
    let Some(&(index, shape)) = self.blocks.get(item_id) else { return Ok(()) };
    if shape == ItemShape::Reasoning {
      self.emit_reasoning_ciphertext(index, item, out);
    }
    if let Some(position) = self.open.iter().position(|open| *open == index) {
      self.open.remove(position);
      out.push(StreamEvent::BlockEnd { index });
    }
    Ok(())
  }

  /// The terminal response event: fail on orphaned content, close every block, report usage, stop.
  fn handle_terminal(&mut self, value: &Value) -> Result<Vec<StreamEvent>, Error> {
    let response = value.get("response").unwrap_or(value);
    if !self.orphans.is_empty() {
      let ids: Vec<&str> = self.orphans.keys().map(String::as_str).collect();
      return Err(Error::Malformed(format!(
        "terminal event with {} buffered delta byte(s) for unannounced item(s): {}",
        self.orphan_bytes,
        ids.join(", ")
      )));
    }
    self.done = true;
    let mut out: Vec<StreamEvent> = {
      let mut open = std::mem::take(&mut self.open);
      open.sort_unstable();
      open.into_iter().map(|index| StreamEvent::BlockEnd { index }).collect()
    };
    if response.get("usage").is_some() {
      out.push(StreamEvent::Usage(buffered::parse_usage(response)));
    }
    let has_tool_uses = self.blocks.values().any(|(_, shape)| *shape == ItemShape::ToolUse);
    out.push(StreamEvent::Stop(buffered::map_stop_reason(response, has_tool_uses)));
    Ok(out)
  }

  fn emit_reasoning_ciphertext(&mut self, index: u32, item: &Value, out: &mut Vec<StreamEvent>) {
    if self.ciphertext_seen.contains(&index) {
      return;
    }
    let Some(encrypted) = item
      .get("encrypted_content")
      .and_then(Value::as_str)
      .filter(|encrypted| !encrypted.is_empty())
    else {
      return;
    };
    self.ciphertext_seen.insert(index);
    out.push(StreamEvent::ReasoningCiphertextDelta { index, ciphertext: encrypted.to_owned() });
  }

  fn flush_orphans(&mut self, item_id: &str, index: u32, out: &mut Vec<StreamEvent>) {
    let Some(buffered) = self.orphans.remove(item_id) else { return };
    for (shape, delta) in buffered {
      self.orphan_bytes -= delta.len();
      self.emit_delta(index, shape, &delta, out);
    }
  }

  fn emit_delta(&self, index: u32, shape: ItemShape, delta: &str, out: &mut Vec<StreamEvent>) {
    match shape {
      ItemShape::Text => out.push(StreamEvent::TextDelta { index, delta: delta.to_owned() }),
      ItemShape::Reasoning => {
        out.push(StreamEvent::ReasoningDelta { index, delta: delta.to_owned() })
      }
      ItemShape::ReasoningSummary => {
        out.push(StreamEvent::ReasoningDisplayDelta { index, delta: delta.to_owned() })
      }
      ItemShape::ToolUse => out.push(StreamEvent::ToolUseDelta {
        index,
        call_id: None,
        name: None,
        arguments: delta.to_owned(),
      }),
    }
  }
}
