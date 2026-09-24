//! Google `interactions` stream wire: the same endpoint with `stream: true`, replying with SSE
//! `event:` lines whose payloads carry the event name again as `event_type`.
//!
//! Conversions:
//! - The event name is read from three places, first wins: the SSE `event:` line, the payload's
//!   `type` field, the payload's `event_type` field.
//! - Steps number blocks in wire order: `step.start` opens the block of a `thought` step and of a
//!   `function_call` step (with the step's `id` / `name` / `arguments`, empty id allowed like the
//!   buffered decoder), while a `model_output` step opens on the text its start carries or, failing
//!   that, on its first delta.
//! - `step.start` carries the opening content of a step (a `summary[]` on a thought, a `content` on
//!   a model output, `arguments` on a call): it streams into the block right away, and the deltas
//!   that follow are the continuations.
//! - `step.delta` streams: `text` (older traffic: `text_delta`) into the step's text block,
//!   `thought_summary` (older traffic: `thought`) into its reasoning block, `thought_signature`
//!   into that block's signature, `arguments_delta` (older traffic: `arguments` carrying
//!   `partial_arguments`) into the step's tool block.
//! - A completed step arrives either in its own `step.stop` event (older traffic: `step.completed`)
//!   or again in the terminal resource's `steps[]`; both fill what the deltas left out, and neither
//!   closes a block: blocks close at the terminal.
//! - A reasoning block reports its proof as a signature at the terminal, from the
//!   `thought_signature` delta when one arrived and otherwise from the completed step, so a thought
//!   that only ever streamed deltas still replays with its signature.
//! - The terminal `interaction.*` event closes every block in index order, reports the resource's
//!   usage and stops with the buffered decoder's status mapping (`interaction.cancelled` is a
//!   `Cancelled` stop). `interaction.failed`, like a bare `error` event, is an upstream error.
//! - The `[DONE]` marker ends the stream; `interaction.started` / `created` / `in_progress` /
//!   `status_update`, unknown and untyped events are tolerated silently: none of them carry output.
//!
//! Trade-offs:
//! - The step `index` of a delta and the position of a step in the terminal resource's `steps[]`
//!   are assumed to be the same coordinate. Events after the terminal fail closed.
//! - A call arrives as `name` + `arguments`; `tool_name` + `args` are read too, which older traffic
//!   spells.
//! - Image and unknown delta shapes are dropped silently: the stream model has no image events.

use std::collections::HashMap;

use serde_json::Value;

use crate::protocol::error::Error;
use crate::protocol::model_use::response::google_interactions as buffered;
use crate::protocol::{BlockKind, StreamEvent};

/// One step's block while the interaction runs.
struct StepBlock {
  index: u32,
  kind: BlockKind,
  /// What the terminal resource must still fill.
  needs_terminal_fill: bool,
  /// The step object the wire sent, from `step.start` or a completed step.
  payload: Option<Value>,
  /// True when the item came from a completed step, rather than only `step.start`.
  completed_payload: bool,
  /// The `thought_signature` delta of this step, which outranks the payload's own signature.
  signature: Option<String>,
}

/// Decodes one `interactions` stream.
#[derive(Default)]
pub struct Decoder {
  next_index: u32,
  steps: HashMap<usize, StepBlock>,
  done: bool,
}

impl Decoder {
  pub fn new() -> Self {
    Self::default()
  }

  /// Feeds one SSE record. The `[DONE]` marker is ignored; the `event:` line wins, and the
  /// payload's `type` then `event_type` field follow, so all wire generations route.
  pub fn feed(&mut self, event: Option<&str>, data: &str) -> Result<Vec<StreamEvent>, Error> {
    if data.trim() == "[DONE]" {
      return Ok(Vec::new());
    }
    if self.done {
      return Err(Error::Malformed("event after the terminal interaction event".to_owned()));
    }
    let value: Value = serde_json::from_str(data)
      .map_err(|error| Error::Malformed(format!("interactions event is not JSON: {error}")))?;
    let event_type = event
      .or_else(|| value.get("type").and_then(Value::as_str))
      .or_else(|| value.get("event_type").and_then(Value::as_str))
      .unwrap_or("");
    let mut out = Vec::new();
    match event_type {
      "interaction.started" | "interaction.created" | "interaction.in_progress" => {}
      "step.start" => self.handle_step_start(&value, &mut out),
      "step.delta" => self.handle_step_delta(&value, &mut out)?,
      "step.stop" | "step.completed" => self.handle_step_stop(&value, &mut out),
      "interaction.completed"
      | "interaction.incomplete"
      | "interaction.budget_exceeded"
      | "interaction.requires_action"
      | "interaction.cancelled" => return self.handle_terminal(&value),
      "interaction.failed" => {
        self.done = true;
        let interaction = value.get("interaction").unwrap_or(&value);
        return Err(buffered::decode_failed_error(interaction));
      }
      "error" => {
        self.done = true;
        return Err(decode_error_event(&value));
      }
      // Unknown and future event types are tolerated.
      _ => {}
    }
    Ok(out)
  }

  fn handle_step_start(&mut self, value: &Value, out: &mut Vec<StreamEvent>) {
    let Some(step) = usize::try_from(value.get("index").and_then(Value::as_u64).unwrap_or(0))
      .ok()
      .and_then(|index| value.get("step").map(|step| (index, step)))
    else {
      return;
    };
    let (index, step) = step;
    match step.get("type").and_then(Value::as_str) {
      // A thought step opens with whatever summary its start carries; the deltas that follow are
      // the continuation, and the signature arrives with them.
      Some("thought") => {
        self.open(index, BlockKind::Reasoning, false, out);
        let text = decode_thought_text(step);
        if let Some(block) = self.steps.get_mut(&index) {
          block.payload = Some(step.clone());
          if !text.is_empty() {
            block.needs_terminal_fill = false;
            let block_index = block.index;
            out.push(StreamEvent::ReasoningDelta { index: block_index, delta: text });
          }
        }
      }
      Some("function_call") => {
        if self.steps.contains_key(&index) {
          return;
        }
        self.open(index, BlockKind::ToolUse, false, out);
        let arguments = step
          .get("arguments")
          .or_else(|| step.get("args"))
          .filter(|arguments| arguments.as_object().is_some_and(|arguments| !arguments.is_empty()))
          .map(Value::to_string)
          .unwrap_or_default();
        if let Some(block) = self.steps.get_mut(&index) {
          block.payload = Some(step.clone());
          if !arguments.is_empty() {
            block.needs_terminal_fill = false;
          }
          let block_index = block.index;
          out.push(StreamEvent::ToolUseDelta {
            index: block_index,
            call_id: Some(step.get("id").and_then(Value::as_str).unwrap_or("").to_owned()),
            name: step
              .get("name")
              .and_then(Value::as_str)
              .filter(|name| !name.is_empty())
              .map(str::to_owned),
            arguments,
          });
        }
      }
      // A model output opens on the text its start carries, or lazily on its first delta.
      Some("model_output") => {
        if let Some(text) = decode_output_text(step).filter(|text| !text.is_empty()) {
          self.open(index, BlockKind::Text, false, out);
          if let Some(block) = self.steps.get_mut(&index) {
            block.payload = Some(step.clone());
            block.needs_terminal_fill = false;
            let block_index = block.index;
            out.push(StreamEvent::TextDelta { index: block_index, delta: text });
          }
        }
      }
      _ => {}
    }
  }

  fn handle_step_delta(&mut self, value: &Value, out: &mut Vec<StreamEvent>) -> Result<(), Error> {
    let Some(index) = usize::try_from(value.get("index").and_then(Value::as_u64).unwrap_or(0))
      .ok()
      .filter(|index| *index != usize::MAX)
    else {
      return Err(Error::Malformed("step.delta index is not a usable step number".to_owned()));
    };
    let delta = value.get("delta");
    match delta.and_then(|delta| delta.get("type")).and_then(Value::as_str) {
      // Current wire spells the model output delta `text`; older traffic spelled it `text_delta`.
      Some("text" | "text_delta") => {
        // A delta that contradicts the step's opened block kind is malformed traffic; tolerate it.
        if self.steps.get(&index).is_some_and(|step| step.kind != BlockKind::Text) {
          return Ok(());
        }
        self.open(index, BlockKind::Text, true, out);
        let text =
          delta.and_then(|delta| delta.get("text")).and_then(Value::as_str).unwrap_or_default();
        if !text.is_empty()
          && let Some(step) = self.steps.get(&index)
        {
          out.push(StreamEvent::TextDelta { index: step.index, delta: text.to_owned() });
        }
      }
      // Thinking text streams on the current wire (`thought_summary`, nested content), while older
      // traffic spelled the delta `thought` with a flat `text`.
      Some("thought_summary" | "thought") => {
        if self.steps.get(&index).is_some_and(|step| step.kind != BlockKind::Reasoning) {
          return Ok(());
        }
        self.open(index, BlockKind::Reasoning, true, out);
        let text = decode_thought_delta_text(delta.expect("checked"));
        if !text.is_empty()
          && let Some(step) = self.steps.get_mut(&index)
        {
          let block_index = step.index;
          out.push(StreamEvent::ReasoningDelta { index: block_index, delta: text });
        }
      }
      // The signature rides its own final delta; a completed thought step does not carry it.
      Some("thought_signature") => {
        if self.steps.get(&index).is_some_and(|step| step.kind != BlockKind::Reasoning) {
          return Ok(());
        }
        self.open(index, BlockKind::Reasoning, true, out);
        if let Some(step) = self.steps.get_mut(&index) {
          step.signature = delta
            .and_then(|delta| delta.get("signature"))
            .and_then(Value::as_str)
            .filter(|signature| !signature.is_empty())
            .map(str::to_owned);
        }
      }
      // Current wire: `arguments` carrying `partial_arguments`; older traffic: `arguments_delta`
      // carrying `arguments`.
      Some("arguments" | "arguments_delta") => {
        if self.steps.get(&index).is_some_and(|step| step.kind != BlockKind::ToolUse) {
          return Ok(());
        }
        self.open(index, BlockKind::ToolUse, true, out);
        let delta = delta.expect("checked");
        let name = delta
          .get("name")
          .and_then(Value::as_str)
          .filter(|name| !name.is_empty())
          .map(str::to_owned);
        let arguments = delta
          .get("partial_arguments")
          .or_else(|| delta.get("arguments"))
          .and_then(Value::as_str)
          .unwrap_or_default()
          .to_owned();
        if let Some(step) = self.steps.get(&index) {
          out.push(StreamEvent::ToolUseDelta { index: step.index, call_id: None, name, arguments });
        }
      }
      // Image and unknown delta shapes are tolerated silently; nothing here models them.
      _ => {}
    }
    Ok(())
  }

  /// The terminal interaction event: fill unstreamed steps, close every block, report, stop.
  fn handle_terminal(&mut self, value: &Value) -> Result<Vec<StreamEvent>, Error> {
    let interaction = value.get("interaction").unwrap_or(value);
    self.done = true;
    let mut out = Vec::new();
    if let Some(steps) = interaction.get("steps").and_then(Value::as_array) {
      for (index, step) in steps.iter().enumerate() {
        if !self.steps.contains_key(&index) {
          // A step that was never announced at all gets its block here.
          match step.get("type").and_then(Value::as_str) {
            Some("thought") => self.open(index, BlockKind::Reasoning, false, &mut out),
            Some("function_call") => self.open(index, BlockKind::ToolUse, false, &mut out),
            Some("model_output") => self.open(index, BlockKind::Text, false, &mut out),
            _ => continue,
          }
        }
        self.fill(index, step, &mut out);
      }
    }
    // A completed thought item retains its original summary part boundaries and image parts for
    // stateless replay. The display text streamed above remains independent from this payload.
    let mut replay_items: Vec<(u32, Value)> = self
      .steps
      .values()
      .filter(|block| block.kind == BlockKind::Reasoning && block.completed_payload)
      .filter_map(|block| {
        let mut item = block.payload.clone()?;
        if let Some(signature) = &block.signature {
          item["signature"] = serde_json::json!(signature);
        }
        Some((block.index, item))
      })
      .collect();
    replay_items.sort_unstable_by_key(|(index, _)| *index);
    out.extend(
      replay_items
        .into_iter()
        .map(|(index, item)| StreamEvent::ReasoningReplayItem { index, item }),
    );
    // Every reasoning block reports its proof before it closes: the signature delta when one came,
    // otherwise the signature the completed step carried.
    let mut proofs: Vec<(u32, String)> = self
      .steps
      .values()
      .filter(|block| block.kind == BlockKind::Reasoning)
      .filter_map(|block| {
        let signature = block
          .signature
          .clone()
          .or_else(|| {
            let step = block.payload.as_ref()?;
            Some(step.get("signature")?.as_str()?.to_owned())
          })
          .filter(|signature| !signature.is_empty())?;
        Some((block.index, signature))
      })
      .collect();
    proofs.sort_unstable_by_key(|(index, _)| *index);
    out.extend(
      proofs
        .into_iter()
        .map(|(index, signature)| StreamEvent::ReasoningSignatureDelta { index, signature }),
    );
    // Close in index order, then report and stop.
    let mut indices: Vec<u32> = self.steps.values().map(|block| block.index).collect();
    indices.sort_unstable();
    out.extend(indices.into_iter().map(|index| StreamEvent::BlockEnd { index }));
    if interaction.get("usage").is_some() {
      out.push(StreamEvent::Usage(buffered::parse_usage(interaction)));
    }
    let has_tool_uses = self.steps.values().any(|block| block.kind == BlockKind::ToolUse);
    out.push(StreamEvent::Stop(buffered::map_stop_reason(
      interaction.get("status").and_then(Value::as_str),
      has_tool_uses,
    )));
    Ok(out)
  }

  /// Opens the step's block unless it exists. `streamed` marks a delta-driven open or update:
  /// once a delta lands, the terminal no longer fills this block.
  fn open(&mut self, index: usize, kind: BlockKind, streamed: bool, out: &mut Vec<StreamEvent>) {
    if let Some(block) = self.steps.get_mut(&index) {
      if streamed {
        block.needs_terminal_fill = false;
      }
      return;
    }
    let block_index = self.next_index;
    self.next_index += 1;
    out.push(StreamEvent::BlockStart { index: block_index, kind });
    self.steps.insert(
      index,
      StepBlock {
        index: block_index,
        kind,
        needs_terminal_fill: !streamed,
        payload: None,
        completed_payload: false,
        signature: None,
      },
    );
  }

  /// Fills what a completed step never streamed and remembers its payload: a step arriving in its
  /// own `step.stop` event and one arriving in the terminal resource take the same route.
  fn fill(&mut self, index: usize, step: &Value, out: &mut Vec<StreamEvent>) {
    let Some(block) = self.steps.get_mut(&index) else { return };
    let block_index = block.index;
    match block.kind {
      BlockKind::Reasoning => {
        block.payload = Some(step.clone());
        block.completed_payload = true;
        // Only a thought that never streamed fills its text here; its signature lands at the
        // terminal.
        if block.needs_terminal_fill {
          let text = decode_thought_text(step);
          if !text.is_empty() {
            out.push(StreamEvent::ReasoningDelta { index: block_index, delta: text });
          }
        }
      }
      BlockKind::ToolUse => {
        let name = step
          .get("name")
          .or_else(|| step.get("tool_name"))
          .and_then(Value::as_str)
          .filter(|name| !name.is_empty())
          .map(str::to_owned);
        if block.needs_terminal_fill {
          out.push(StreamEvent::ToolUseDelta {
            index: block_index,
            call_id: Some(step.get("id").and_then(Value::as_str).unwrap_or("").to_owned()),
            name,
            arguments: step
              .get("arguments")
              .or_else(|| step.get("args"))
              .cloned()
              .unwrap_or_default()
              .to_string(),
          });
        } else if let Some(name) = name {
          out.push(StreamEvent::ToolUseDelta {
            index: block_index,
            call_id: None,
            name: Some(name),
            arguments: String::new(),
          });
        }
      }
      BlockKind::Text => {
        if block.needs_terminal_fill
          && let Some(text) = decode_output_text(step)
        {
          out.push(StreamEvent::TextDelta { index: block_index, delta: text });
        }
      }
    }
  }

  /// One step's completed payload arriving in its own event. The event may carry only the index.
  fn handle_step_stop(&mut self, value: &Value, out: &mut Vec<StreamEvent>) {
    let Some(index) = usize::try_from(value.get("index").and_then(Value::as_u64).unwrap_or(0)).ok()
    else {
      return;
    };
    let Some(step) = value.get("step") else { return };
    if !self.steps.contains_key(&index) {
      match step.get("type").and_then(Value::as_str) {
        Some("thought") => self.open(index, BlockKind::Reasoning, false, out),
        Some("function_call") => self.open(index, BlockKind::ToolUse, false, out),
        Some("model_output") => self.open(index, BlockKind::Text, false, out),
        _ => return,
      }
    }
    self.fill(index, step, out);
  }
}

/// The text of a thought delta: the current wire nests the summary content under `content` (one
/// text block, or an array of them), while older traffic put the text in `text` directly.
fn decode_thought_delta_text(delta: &Value) -> String {
  let content = delta.get("content");
  let blocks: Vec<&Value> = match content {
    Some(Value::Object(_)) => content.into_iter().collect(),
    Some(Value::Array(blocks)) => blocks.iter().collect(),
    _ => Vec::new(),
  };
  let text: Vec<&str> = blocks
    .iter()
    .filter(|block| block.get("type").and_then(Value::as_str) != Some("image"))
    .filter_map(|block| block.get("text").and_then(Value::as_str))
    .filter(|text| !text.is_empty())
    .collect();
  if !text.is_empty() {
    return text.join("\n");
  }
  delta.get("text").and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// The text of a thought step: `summary[].text` first, the `content` string as fallback.
fn decode_thought_text(step: &Value) -> String {
  let summary: Vec<&str> = step
    .get("summary")
    .and_then(Value::as_array)
    .map(|entries| {
      entries.iter().filter_map(|entry| entry.get("text").and_then(Value::as_str)).collect()
    })
    .unwrap_or_default();
  if !summary.is_empty() {
    return summary.join("\n");
  }
  step.get("content").and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// The text of a model output step, joining its text blocks.
fn decode_output_text(step: &Value) -> Option<String> {
  match step.get("content") {
    Some(Value::String(text)) => Some(text.clone()),
    Some(Value::Array(blocks)) => Some(
      blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n"),
    ),
    _ => None,
  }
}

/// Maps a bare `error` event to an upstream error.
fn decode_error_event(value: &Value) -> Error {
  let error = value.get("error");
  let message = error
    .and_then(|error| error.get("message"))
    .and_then(Value::as_str)
    .unwrap_or("upstream error event");
  let code = error
    .and_then(|error| error.get("code").or_else(|| error.get("status")))
    .and_then(Value::as_str);
  Error::from_in_band(code.map(str::to_owned), message.to_owned())
}
