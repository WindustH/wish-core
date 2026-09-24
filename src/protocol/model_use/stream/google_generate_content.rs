//! Google `generateContent` stream wire (`:streamGenerateContent?alt=sse`).
//!
//! Conversions:
//! - The wire has no block events and no terminal marker: the body ending IS the terminal. Parts
//!   number blocks as they arrive: unsigned thought chunks continue one reasoning block, a signed
//!   thought closes before the next thought part, and consecutive text chunks share a text block.
//! - A `thought: true` part streams reasoning text; its `thoughtSignature` is held back and emitted
//!   as that block's proof when it closes.
//! - A signature riding on a text or `functionCall` part becomes a signature-only reasoning block
//!   of its own, emitted after the open text block closes and before the part it belongs to opens
//!   its own block, so the request side can attach it to the next part again.
//! - A signature arriving on its own - a bare `thoughtSignature` part, or a `thought` part without
//!   text - fills the open reasoning block when it carries no signature yet, and otherwise becomes
//!   one closed signature-only reasoning block, so the mandatory signature replay never loses it.
//! - A `functionCall` part is one complete block: it opens, carries its call get_id (empty when the
//!   wire omits `id`, the pairing-by-name convention of the buffered decoder), name and serialized
//!   `args`, and closes in place.
//! - `usageMetadata` rides every chunk cumulatively and is reported whenever it is non-zero;
//!   `finishReason` is remembered for the terminal. `finish()` closes the open blocks, maps the
//!   reason through the buffered decoder's mapping, and stops; a stream that ends without a
//!   finish reason is only tolerable when a tool call was made.
//!
//! Trade-offs:
//! - An `inlineData` part and unknown part shapes are tolerated silently (the event model has no
//!   image stream yet); an `error` object in a chunk maps to an upstream error exactly like the
//!   buffered decoder.

use serde_json::{Value, json};

use crate::protocol::error::Error;
use crate::protocol::model_use::response::google_generate_content as buffered;
use crate::protocol::{BlockKind, StreamEvent};

/// One reasoning block while it is open, holding its signature back until it closes.
struct OpenReasoning {
  index: u32,
  signature: Option<String>,
}

/// Decodes one `generateContent` stream.
#[derive(Default)]
pub struct Decoder {
  next_index: u32,
  open_text: Option<u32>,
  open_reasoning: Option<OpenReasoning>,
  finish_reason: Option<String>,
  tool_uses: u32,
}

impl Decoder {
  pub fn new() -> Self {
    Self::default()
  }

  /// Feeds one SSE record. The `event:` name is always absent on this wire.
  pub fn feed(&mut self, _event: Option<&str>, data: &str) -> Result<Vec<StreamEvent>, Error> {
    let chunk: Value = serde_json::from_str(data)
      .map_err(|error| Error::Malformed(format!("generateContent chunk is not JSON: {error}")))?;
    if chunk.get("error").is_some_and(|error| error.is_object()) {
      return Err(buffered::decode_upstream_error(&chunk));
    }
    let mut out = Vec::new();
    if chunk.get("usageMetadata").is_some_and(|usage| !usage.is_null()) {
      let usage = buffered::parse_usage(&chunk);
      if usage.total_tokens.unwrap_or(0) > 0 {
        out.push(StreamEvent::Usage(usage));
      }
    }
    let candidate = chunk.pointer("/candidates/0");
    if let Some(finish) =
      candidate.and_then(|candidate| candidate.get("finishReason")).and_then(Value::as_str)
    {
      self.finish_reason = Some(finish.to_owned());
    }
    if let Some(parts) =
      candidate.and_then(|candidate| candidate.pointer("/content/parts")).and_then(Value::as_array)
    {
      for part in parts {
        self.decode_part(part, &mut out)?;
      }
    }
    Ok(out)
  }

  /// The body ended: close the open blocks, map the remembered reason, stop. A stream that ends
  /// without a finish reason is only tolerable when a tool call was made.
  pub fn finish(&mut self) -> Result<Vec<StreamEvent>, Error> {
    let mut out = Vec::new();
    self.close_text(&mut out);
    self.close_reasoning(&mut out);
    let reason = match self.finish_reason.as_deref() {
      Some(finish) => buffered::map_stop_reason(Some(finish), self.tool_uses > 0),
      None if self.tool_uses > 0 => crate::protocol::StopReason::ToolUse,
      None => {
        return Err(Error::Malformed("stream ended without a finishReason".to_owned()));
      }
    };
    out.push(StreamEvent::Stop(reason));
    Ok(out)
  }

  fn decode_part(&mut self, part: &Value, out: &mut Vec<StreamEvent>) -> Result<(), Error> {
    let signature = part
      .get("thoughtSignature")
      .and_then(Value::as_str)
      .filter(|signature| !signature.is_empty());
    if part.get("thought") == Some(&Value::Bool(true)) {
      if let Some(text) = part.get("text").and_then(Value::as_str) {
        self.close_text(out);
        // A signature seals the previous thought part. Reusing its block for the next part would
        // concatenate their text and replace the first proof with the second one.
        if self.open_reasoning.as_ref().is_some_and(|open| open.signature.is_some()) {
          self.close_reasoning(out);
        }
        let index = match &mut self.open_reasoning {
          Some(open) => open.index,
          None => {
            let index = self.next_index;
            self.next_index += 1;
            self.open_reasoning = Some(OpenReasoning { index, signature: None });
            out.push(StreamEvent::BlockStart { index, kind: BlockKind::Reasoning });
            index
          }
        };
        if !text.is_empty() {
          out.push(StreamEvent::ReasoningDelta { index, delta: text.to_owned() });
        }
        if signature.is_some() {
          self.open_reasoning.as_mut().expect("open block").signature =
            signature.map(str::to_owned);
        }
        return Ok(());
      }
      // A thought part without text can still close the signature.
      if let Some(signature) = signature {
        self.land_signature(signature, out);
      }
      return Ok(());
    }
    if let Some(function_call) = part.get("functionCall") {
      self.close_text(out);
      self.close_reasoning(out);
      if let Some(signature) = signature {
        self.signature_only(signature, out);
      }
      let name = function_call
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| Error::Malformed("functionCall part is missing `name`".to_owned()))?;
      let call_id = function_call.get("id").and_then(Value::as_str).unwrap_or("");
      let arguments = function_call.get("args").cloned().unwrap_or_else(|| json!({}));
      let index = self.next_index;
      self.next_index += 1;
      self.tool_uses += 1;
      out.push(StreamEvent::BlockStart { index, kind: BlockKind::ToolUse });
      out.push(StreamEvent::ToolUseDelta {
        index,
        call_id: Some(call_id.to_owned()),
        name: Some(name.to_owned()),
        arguments: arguments.to_string(),
      });
      out.push(StreamEvent::BlockEnd { index });
      return Ok(());
    }
    if let Some(text) = part.get("text").and_then(Value::as_str) {
      self.close_reasoning(out);
      if let Some(signature) = signature {
        // The signature belongs to this part: close what streamed before it, announce the
        // signature, then let this part's text open a fresh block after it.
        self.close_text(out);
        self.signature_only(signature, out);
      }
      if !text.is_empty() {
        let index = match self.open_text {
          Some(index) => index,
          None => {
            let index = self.next_index;
            self.next_index += 1;
            self.open_text = Some(index);
            out.push(StreamEvent::BlockStart { index, kind: BlockKind::Text });
            index
          }
        };
        out.push(StreamEvent::TextDelta { index, delta: text.to_owned() });
      }
      return Ok(());
    }
    // A part that carries nothing but a signature still lands: fill the open reasoning block
    // when it has no signature yet, otherwise announce a signature-only block of its own.
    if let Some(signature) = signature {
      self.land_signature(signature, out);
      return Ok(());
    }
    // `inlineData` parts and unknown shapes are tolerated; images are not streamable here yet.
    Ok(())
  }

  /// A signature that arrives on its own: the open reasoning block takes it when it carries no
  /// signature yet, else it becomes one closed signature-only reasoning block.
  fn land_signature(&mut self, signature: &str, out: &mut Vec<StreamEvent>) {
    if let Some(open) = &mut self.open_reasoning
      && open.signature.is_none()
    {
      open.signature = Some(signature.to_owned());
      return;
    }
    self.close_reasoning(out);
    self.signature_only(signature, out);
  }

  /// A signature riding on someone else's part: one closed reasoning block carrying only a proof.
  fn signature_only(&mut self, signature: &str, out: &mut Vec<StreamEvent>) {
    let index = self.next_index;
    self.next_index += 1;
    out.push(StreamEvent::BlockStart { index, kind: BlockKind::Reasoning });
    out.push(StreamEvent::ReasoningSignatureDelta { index, signature: signature.to_owned() });
    out.push(StreamEvent::BlockEnd { index });
  }

  fn close_text(&mut self, out: &mut Vec<StreamEvent>) {
    if let Some(index) = self.open_text.take() {
      out.push(StreamEvent::BlockEnd { index });
    }
  }

  fn close_reasoning(&mut self, out: &mut Vec<StreamEvent>) {
    if let Some(open) = self.open_reasoning.take() {
      if let Some(signature) = open.signature {
        out.push(StreamEvent::ReasoningSignatureDelta { index: open.index, signature });
      }
      out.push(StreamEvent::BlockEnd { index: open.index });
    }
  }
}
