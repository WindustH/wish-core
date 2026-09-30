//! Mistral's content chunks, the one shape its two wires share for text and thoughts.
//!
//! Both the chat wire in its Mistral mode and the conversations wire carry a message's content as a
//! plain string or as a list of typed chunks: `text` chunks with a `text` member, and `thinking`
//! chunks whose `thinking` member is itself a list of text parts. A replayed thought travels back
//! as one such chunk. The renderers, the body readers and the stream decoders of both wires read
//! and write them through this module, so the two wires cannot drift apart.

use serde_json::{Value, json};

use crate::protocol::error::Error;

/// The chunk a replayed thought travels in.
pub(crate) fn render_thinking_chunk(reasoning: &str) -> Value {
  json!({
    "type": "thinking",
    "closed": true,
    "thinking": [{ "type": "text", "text": reasoning }],
  })
}

/// The `type` of one content chunk.
pub(crate) fn chunk_type(chunk: &Value) -> Option<&str> {
  chunk.get("type").and_then(Value::as_str)
}

/// Whether a content chunk is a thought: the vendor's schema spells the type `thinking`, its prose
/// spells it `think`, so both are read.
pub(crate) fn is_thinking_chunk(chunk: &Value) -> bool {
  matches!(chunk_type(chunk), Some("thinking" | "think"))
}

/// One piece of text a chunk list carries, and which kind of content it is.
pub(crate) enum ChunkText<'a> {
  /// The text of a `text` chunk.
  Text(&'a str),
  /// The text of one part of a thought chunk.
  Thought(&'a str),
}

/// Visits the text of a chunk list in order: every `text` chunk's text, and every text part of every
/// thought chunk. Empty texts are visited too; other chunk types are skipped. A thought chunk
/// without its list of parts is malformed.
pub(crate) fn visit_chunks<'a>(
  chunks: &'a [Value],
  mut visit: impl FnMut(ChunkText<'a>),
) -> Result<(), Error> {
  for chunk in chunks {
    if chunk_type(chunk) == Some("text") {
      if let Some(text) = chunk.get("text").and_then(Value::as_str) {
        visit(ChunkText::Text(text));
      }
      continue;
    }
    if !is_thinking_chunk(chunk) {
      continue;
    }
    let parts = chunk
      .get("thinking")
      .and_then(Value::as_array)
      .ok_or_else(|| Error::Malformed("`thinking` chunk carries no `thinking` list".to_owned()))?;
    for part in parts {
      if let Some(text) = part.get("text").and_then(Value::as_str) {
        visit(ChunkText::Thought(text));
      }
    }
  }
  Ok(())
}

/// The text chunks of a chunk list, joined as one text.
pub(crate) fn join_text_chunks(chunks: &[Value]) -> String {
  chunks
    .iter()
    .filter(|chunk| chunk_type(chunk) == Some("text"))
    .filter_map(|chunk| chunk.get("text").and_then(Value::as_str))
    .collect()
}

/// The thought chunks of a chunk list, joined as one text.
pub(crate) fn join_thinking_chunks(chunks: &[Value]) -> Result<String, Error> {
  let mut plaintext = String::new();
  visit_chunks(chunks, |text| {
    if let ChunkText::Thought(text) = text {
      plaintext.push_str(text);
    }
  })?;
  Ok(plaintext)
}
