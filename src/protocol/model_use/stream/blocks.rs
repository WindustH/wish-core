//! Block bookkeeping the stream decoders share: handing out indices, the lazily opened text and
//! reasoning blocks of a wire without block events, and closing what is still open at the end.

use serde_json::Value;

use crate::protocol::error::Error;
use crate::protocol::model_use::mistral_chunks::{ChunkText, visit_chunks};
use crate::protocol::{BlockKind, StreamEvent};

/// Hands out block indices in the order blocks open, for a wire that does not number its blocks.
#[derive(Default)]
pub(super) struct BlockCounter {
  next: u32,
}

impl BlockCounter {
  /// The next index, for an item that takes its place in the sequence without opening a block.
  pub(super) fn allocate(&mut self) -> u32 {
    let index = self.next;
    self.next += 1;
    index
  }

  /// Opens the next block: allocates its index and announces it with its kind.
  pub(super) fn open(&mut self, kind: BlockKind, out: &mut Vec<StreamEvent>) -> u32 {
    let index = self.allocate();
    out.push(StreamEvent::BlockStart { index, kind });
    index
  }
}

/// The one text block and the one reasoning block of a turn on a wire without block events (the
/// chat wire and Mistral's conversations wire): each opens the first time its kind streams, and
/// both stay open until the turn ends. Tool blocks take their indices from the same counter.
#[derive(Default)]
pub(super) struct LazyBlocks {
  pub(super) counter: BlockCounter,
  text: Option<u32>,
  reasoning: Option<u32>,
}

impl LazyBlocks {
  /// Streams one text delta, opening the text block on the first; an empty delta, which a chunk
  /// list is full of, is ignored.
  pub(super) fn push_text(&mut self, text: &str, out: &mut Vec<StreamEvent>) {
    if text.is_empty() {
      return;
    }
    let index = match self.text {
      Some(index) => index,
      None => *self.text.insert(self.counter.open(BlockKind::Text, out)),
    };
    out.push(StreamEvent::TextDelta { index, delta: text.to_owned() });
  }

  /// Streams one reasoning delta, opening the reasoning block on the first; an empty delta is
  /// ignored.
  pub(super) fn push_reasoning(&mut self, text: &str, out: &mut Vec<StreamEvent>) {
    if text.is_empty() {
      return;
    }
    let index = match self.reasoning {
      Some(index) => index,
      None => *self.reasoning.insert(self.counter.open(BlockKind::Reasoning, out)),
    };
    out.push(StreamEvent::ReasoningDelta { index, delta: text.to_owned() });
  }

  /// Streams a Mistral content chunk list: text chunks as text, thought parts as reasoning.
  pub(super) fn push_chunks(
    &mut self,
    chunks: &[Value],
    out: &mut Vec<StreamEvent>,
  ) -> Result<(), Error> {
    visit_chunks(chunks, |text| match text {
      ChunkText::Text(text) => self.push_text(text, out),
      ChunkText::Thought(text) => self.push_reasoning(text, out),
    })
  }

  /// The text and reasoning blocks that opened.
  pub(super) fn get_opened(&self) -> impl Iterator<Item = u32> {
    self.text.into_iter().chain(self.reasoning)
  }
}

/// Closes the given blocks in index order, once each.
pub(super) fn end_blocks_in_order(mut indices: Vec<u32>, out: &mut Vec<StreamEvent>) {
  indices.sort_unstable();
  indices.dedup();
  out.extend(indices.into_iter().map(|index| StreamEvent::BlockEnd { index }));
}
