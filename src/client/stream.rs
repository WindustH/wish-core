//! One streamed call in flight: the body's records decoded into the protocol's normalized events.
use super::AttemptObserver;
use crate::{
  Error,
  executor::model::ModelStream,
  protocol::{
    StreamAccumulator, StreamEvent,
    account_state::AccountState,
    attempt::ReplyStream,
    model_use::{ModelUseProtocol, stream::StreamDecoder},
  },
  transport::RecordStream,
};
use std::collections::VecDeque;

pub struct EventStream<S: ReplyStream> {
  observer: Option<Box<dyn AttemptObserver>>,
  protocol: ModelUseProtocol,
  records: RecordStream<S>,
  decoder: StreamDecoder,
  pending: VecDeque<StreamEvent>,
  done: bool,
  /// The reading the reply head carried.
  pub(super) account_state: Option<AccountState>,
}

impl<S: ReplyStream> EventStream<S> {
  pub(super) fn new(
    records: RecordStream<S>,
    protocol: ModelUseProtocol,
    observer: Option<Box<dyn AttemptObserver>>,
    account_state: Option<AccountState>,
  ) -> Self {
    Self {
      observer,
      protocol,
      records,
      decoder: protocol.create_stream_decoder(),
      pending: VecDeque::new(),
      done: false,
      account_state,
    }
  }

  /// A caller-owned accumulator with this wire's interruption replay rules and reply metadata.
  pub fn create_accumulator(&self) -> StreamAccumulator {
    StreamAccumulator::for_protocol(self.protocol).with_account_state(self.account_state.clone())
  }

  /// Abandon local reading without flushing parsers or synthesizing a normal EOF.
  /// This does not confirm remote cancellation or stop upstream billing.
  pub fn abort(self) {
    drop(self);
  }

  /// The next normalized event, or `None` at the end of the stream.
  ///
  /// Reading stops at the protocol's terminal event even if the body keeps going: nothing after it
  /// is trustworthy, and the accumulator would reject it anyway.
  pub async fn next(&mut self) -> Result<Option<StreamEvent>, Error> {
    loop {
      if let Some(event) = self.pending.pop_front() {
        return Ok(Some(event));
      }
      if self.done {
        return Ok(None);
      }
      match self.records.next().await? {
        Some(record) => {
          let events = self.decoder.feed(record.event.as_deref(), &record.data)?;
          for event in &events {
            if let Some(observer) = &self.observer {
              observer.observe(event);
            }
          }
          if events.iter().any(|event| matches!(event, StreamEvent::Stop(_))) {
            self.done = true;
            self.observer.take();
          }
          self.pending.extend(events);
        }
        None => {
          let events = self.decoder.finish()?;
          self.observer.take();
          self.pending.extend(events);
          self.done = true;
        }
      }
    }
  }

  /// Reads the first event ahead, to be handed over first. Until it is in hand the attempt has
  /// delivered nothing, so a failure here may still replace it.
  pub(super) async fn read_ahead(&mut self) -> Result<(), Error> {
    if let Some(event) = self.next().await? {
      self.pending.push_front(event);
    }
    Ok(())
  }
}

impl<S: ReplyStream> ModelStream for EventStream<S> {
  fn create_accumulator(&self) -> StreamAccumulator {
    EventStream::create_accumulator(self)
  }
  async fn next(&mut self) -> Result<Option<StreamEvent>, Error> {
    EventStream::next(self).await
  }
  fn abort(self) {
    EventStream::abort(self);
  }
}
