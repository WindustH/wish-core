//! Anthropic Messages stream wire.
//!
//! Conversions:
//! - The event contract is `message_start`, then `content_block_start` / `content_block_delta` /
//!   `content_block_stop` per content block, then `message_delta` and `message_stop`, with `ping`
//!   heartbeats and a terminal `error` event.
//! - The wire's block index becomes the block index. `signature_delta` and `redacted_thinking` data
//!   land in `ReasoningOpaque`; usage merges into a running total and is re-emitted whole, because
//!   the accumulator replaces rather than merges; the stop reason mapping is the buffered one.
//!
//! Trade-offs:
//! - The `input` object of a `tool_use` block start is ignored: the real arguments arrive as
//!   `input_json_delta` fragments.
//! - `message_stop` without a stop reason seen maps to `Unknown`, exactly like a buffered body
//!   without one, rather than guessing from the blocks.

use serde_json::Value;

use crate::protocol::error::Error;
use crate::protocol::model_use::response::anthropic_messages as buffered;
use crate::protocol::{BlockKind, StopReason, StreamEvent, Usage};

/// Decodes one Anthropic stream.
#[derive(Default)]
pub struct Decoder {
  usage: Usage,
  stop_reason: Option<StopReason>,
}

impl Decoder {
  pub fn new() -> Self {
    Self::default()
  }

  /// Feeds one SSE record.
  pub fn feed(&mut self, event: Option<&str>, data: &str) -> Result<Vec<StreamEvent>, Error> {
    let payload: Value = serde_json::from_str(data)
      .map_err(|error| Error::Malformed(format!("anthropic event is not JSON: {error}")))?;
    // The payload's own `type` is authoritative; the `event:` line is a courtesy.
    let kind = payload.get("type").and_then(Value::as_str).or(event).unwrap_or_default();
    let mut out = Vec::new();
    match kind {
      "message_start" => {
        let usage = payload.pointer("/message/usage");
        if usage.is_some() {
          self.merge_usage(usage);
          out.push(StreamEvent::Usage(self.usage));
        }
      }
      "content_block_start" => {
        let index = parse_block_index(&payload, "content_block_start")?;
        let block = payload.get("content_block");
        let field = |name: &str| block.and_then(|block| block.get(name)).and_then(Value::as_str);
        match field("type").unwrap_or_default() {
          "text" => out.push(StreamEvent::BlockStart { index, kind: BlockKind::Text }),
          "thinking" => out.push(StreamEvent::BlockStart { index, kind: BlockKind::Reasoning }),
          "redacted_thinking" => {
            out.push(StreamEvent::BlockStart { index, kind: BlockKind::Reasoning });
            out.push(StreamEvent::ReasoningCiphertextDelta {
              index,
              ciphertext: field("data").unwrap_or_default().to_owned(),
            });
          }
          "tool_use" => {
            let call_id = field("id")
              .ok_or_else(|| Error::Malformed("tool_use block is missing `id`".to_owned()))?
              .to_owned();
            let name = field("name")
              .ok_or_else(|| Error::Malformed("tool_use block is missing `name`".to_owned()))?
              .to_owned();
            out.push(StreamEvent::BlockStart { index, kind: BlockKind::ToolUse });
            out.push(StreamEvent::ToolUseDelta {
              index,
              call_id: Some(call_id),
              name: Some(name),
              arguments: String::new(),
            });
          }
          _ => {}
        }
      }
      "content_block_delta" => {
        let index = parse_block_index(&payload, "content_block_delta")?;
        let delta = payload.get("delta");
        let field = |name: &str| {
          delta
            .and_then(|delta| delta.get(name))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
        };
        let kind = delta.and_then(|delta| delta.get("type")).and_then(Value::as_str);
        match kind.unwrap_or_default() {
          "text_delta" => out.push(StreamEvent::TextDelta { index, delta: field("text") }),
          "thinking_delta" => {
            out.push(StreamEvent::ReasoningDelta { index, delta: field("thinking") })
          }
          "signature_delta" => {
            out.push(StreamEvent::ReasoningSignatureDelta { index, signature: field("signature") })
          }
          "input_json_delta" => out.push(StreamEvent::ToolUseDelta {
            index,
            call_id: None,
            name: None,
            arguments: field("partial_json"),
          }),
          _ => {}
        }
      }
      "content_block_stop" => {
        out.push(StreamEvent::BlockEnd {
          index: parse_block_index(&payload, "content_block_stop")?,
        });
      }
      "message_delta" => {
        if let Some(reason) = payload.pointer("/delta/stop_reason").and_then(Value::as_str) {
          self.stop_reason = Some(buffered::map_stop_reason(Some(reason)));
        }
        let usage = payload.get("usage");
        if usage.is_some() {
          self.merge_usage(usage);
          out.push(StreamEvent::Usage(self.usage));
        }
      }
      "message_stop" => {
        let reason = self.stop_reason.take().unwrap_or_else(|| buffered::map_stop_reason(None));
        out.push(StreamEvent::Stop(reason));
      }
      "error" => {
        let error = payload.get("error");
        return Err(Error::from_in_band(
          error.and_then(|error| error.get("type")).and_then(Value::as_str).map(str::to_owned),
          error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("upstream reported an error without a message")
            .to_owned(),
        ));
      }
      _ => {}
    }
    Ok(out)
  }

  /// Merges one wire usage object into the running total, folding input exactly like the buffered
  /// decoder: `input_tokens` plus both cache counters, total derived once both sides are known.
  fn merge_usage(&mut self, usage: Option<&Value>) {
    let Some(usage) = usage else { return };
    let field = |name: &str| usage.get(name).and_then(Value::as_u64);
    let cached = field("cache_read_input_tokens");
    let cache_write = field("cache_creation_input_tokens");
    if let Some(cached) = cached {
      self.usage.cached_input_tokens = Some(cached);
    }
    if let Some(cache_write) = cache_write {
      self.usage.cache_write_input_tokens = Some(cache_write);
    }
    if let Some(input) = field("input_tokens") {
      self.usage.input_tokens =
        Some(input.saturating_add(cached.unwrap_or(0)).saturating_add(cache_write.unwrap_or(0)));
    }
    if let Some(output) = field("output_tokens") {
      self.usage.output_tokens = Some(output);
    }
    if let Some(thinking) =
      usage.pointer("/output_tokens_details/thinking_tokens").and_then(Value::as_u64)
    {
      self.usage.reasoning_tokens = Some(thinking);
    }
    if let (Some(input), Some(output)) = (self.usage.input_tokens, self.usage.output_tokens) {
      self.usage.total_tokens = Some(input.saturating_add(output));
    }
  }
}

/// The wire block index of an event that names a block.
fn parse_block_index(payload: &Value, event: &str) -> Result<u32, Error> {
  payload
    .get("index")
    .and_then(Value::as_u64)
    .and_then(|index| u32::try_from(index).ok())
    .ok_or_else(|| Error::Malformed(format!("{event} is missing a usable `index`")))
}
