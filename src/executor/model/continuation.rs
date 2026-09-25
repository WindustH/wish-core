//! Request-local output continuation. No synthetic instructions or intermediate responses enter Session.
use crate::session::statistics::CallObservation;
use crate::{
  Error,
  protocol::{ContentBlock, Message, Request, StreamEvent, Usage},
};

pub(in crate::executor) fn combine_usage(previous: Option<Usage>, current: Usage) -> Usage {
  let Some(previous) = previous else {
    return current;
  };
  fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    a?.checked_add(b?)
  }
  Usage {
    input_tokens: add(previous.input_tokens, current.input_tokens),
    cached_input_tokens: add(previous.cached_input_tokens, current.cached_input_tokens),
    cache_write_input_tokens: add(
      previous.cache_write_input_tokens,
      current.cache_write_input_tokens,
    ),
    output_tokens: add(previous.output_tokens, current.output_tokens),
    reasoning_tokens: add(previous.reasoning_tokens, current.reasoning_tokens),
    total_tokens: add(previous.total_tokens, current.total_tokens),
  }
}

pub(in crate::executor) struct Continuation {
  pub request: Request,
  pub messages: Vec<Message>,
  pub usage: Option<Usage>,
  pub next_index: u32,
}
impl Continuation {
  pub fn new(request: &Request) -> Self {
    Self { request: request.clone(), messages: Vec::new(), usage: None, next_index: 0 }
  }
  pub fn extend(&mut self, messages: Vec<Message>) {
    self.request.conversation.extend(messages.clone());
    self.messages.extend(messages);
    self.request.conversation.push(Message::User {
      metadata: Default::default(),
      content: vec![ContentBlock::Text { text: "Your previous response reached its output limit. Continue from where it stopped without repeating content already provided. Reissue any cut-off tool call in full if still needed.".into() }],
    });
  }
}

pub(super) struct SegmentObservation {
  pub call: CallObservation,
  pub previous_usage: Option<Usage>,
  pub index_offset: u32,
  pub next_index: u32,
}
impl std::ops::Deref for SegmentObservation {
  type Target = CallObservation;
  fn deref(&self) -> &CallObservation {
    &self.call
  }
}
impl std::ops::DerefMut for SegmentObservation {
  fn deref_mut(&mut self) -> &mut CallObservation {
    &mut self.call
  }
}
impl SegmentObservation {
  /// Observers see unique block indices, cumulative usage, and only the final stop event.
  pub fn map_event(&mut self, mut event: StreamEvent) -> Result<Option<StreamEvent>, Error> {
    match &mut event {
      StreamEvent::Stop(crate::protocol::StopReason::MaxOutputLengthExceeded) => return Ok(None),
      StreamEvent::Usage(usage) => *usage = combine_usage(self.previous_usage, *usage),
      StreamEvent::BlockStart { index, .. }
      | StreamEvent::TextDelta { index, .. }
      | StreamEvent::ReasoningDelta { index, .. }
      | StreamEvent::ReasoningDisplayDelta { index, .. }
      | StreamEvent::ReasoningSignatureDelta { index, .. }
      | StreamEvent::ReasoningCiphertextDelta { index, .. }
      | StreamEvent::ReasoningReplayItem { index, .. }
      | StreamEvent::ToolUseDelta { index, .. }
      | StreamEvent::BlockEnd { index }
      | StreamEvent::BlockComplete { index }
      | StreamEvent::UpstreamCompaction { index, .. } => {
        *index = index
          .checked_add(self.index_offset)
          .ok_or_else(|| Error::Malformed("continuation block index overflow".into()))?;
        self.next_index = self.next_index.max(
          index
            .checked_add(1)
            .ok_or_else(|| Error::Malformed("continuation block index overflow".into()))?,
        );
      }
      _ => {}
    }
    Ok(Some(event))
  }
}
