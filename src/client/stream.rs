//! One streamed call in flight: the body's records decoded into the protocol's normalized events.
use super::{AttemptObserver, stream_recovery::Recovery};
use crate::{
  executor::model::ModelStream,
  protocol::error::Error,
  protocol::{
    StreamAccumulator, StreamEvent,
    account_state::AccountState,
    attempt::{ReplyStream, Transport},
    model_use::{ModelUseProtocol, stream::StreamDecoder},
  },
  transport::RecordStream,
};
use std::collections::VecDeque;

pub struct EventStream<T: Transport> {
  pub(super) recovery: Option<Recovery>,
  transport: std::sync::Arc<T>,
  observer: Option<Box<dyn AttemptObserver>>,
  protocol: ModelUseProtocol,
  records: Option<RecordStream<T::Stream>>,
  decoder: StreamDecoder,
  pending: VecDeque<StreamEvent>,
  done: bool,
  /// The reading the reply head carried.
  pub(super) account_state: Option<AccountState>,
}

impl<T: Transport + Send + Sync> EventStream<T> {
  pub(super) fn new(
    transport: std::sync::Arc<T>,
    records: RecordStream<T::Stream>,
    protocol: ModelUseProtocol,
    observer: Option<Box<dyn AttemptObserver>>,
    account_state: Option<AccountState>,
  ) -> Self {
    Self {
      transport,
      recovery: None,
      observer,
      protocol,
      records: Some(records),
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
      if self.recovery.as_ref().is_some_and(|r| r.waiting.is_some()) {
        self.reopen().await?;
        continue;
      }
      if self.recovery.as_ref().is_some_and(|r| r.continuation) {
        self.schedule_recovery(None)?;
        continue;
      }
      let next = match self.records.as_mut().expect("open stream").next().await {
        Ok(next) => next,
        Err(error) => {
          self.schedule_recovery(Some(error))?;
          continue;
        }
      };
      match next {
        Some(record) => {
          let (data, checkpoint, terminal) = if let Some(recovery) = &self.recovery {
            recovery.protocol.inspect(&record.data)?
          } else {
            (record.data, None, false)
          };
          let mut events = match self.decoder.feed(record.event.as_deref(), &data) {
            Ok(events) => events,
            Err(error) => {
              self.schedule_recovery(Some(error))?;
              continue;
            }
          };
          for event in &events {
            if let Some(observer) = &self.observer {
              observer.observe(event);
            }
          }
          if let Some(recovery) = &mut self.recovery {
            recovery.terminal |= terminal;
            for event in &mut events {
              if let StreamEvent::Usage(usage) = event {
                recovery.usage = *usage;
                if recovery.initial_input.is_none() {
                  recovery.initial_input = usage.input_tokens;
                }
                *usage =
                  crate::protocol::model_use::recovery::sum_usage(recovery.previous_usage, *usage);
              }
            }
            if let Some(tokens) = recovery.initial_input {
              events.push(StreamEvent::InputTokens { tokens });
            }
            if let Some(checkpoint) = checkpoint {
              recovery.token = Some(checkpoint.token);
              recovery.decoder = self.decoder.checkpoint();
              recovery.revision += 1;
              recovery.failures = 0;
              recovery.committed_usage = recovery.usage;
              recovery.continuation = checkpoint.continuation;
              events.push(StreamEvent::Checkpoint { revision: recovery.revision });
            }
          }
          if events.iter().any(|event| matches!(event, StreamEvent::Stop(_))) {
            self.done = true;
            self.observer.take();
          }
          self.pending.extend(events);
        }
        None => {
          if self.recovery.as_ref().is_some_and(|r| r.token.is_some() && !r.terminal) {
            self.schedule_recovery(None)?;
            continue;
          }
          let events = self.decoder.finish()?;
          self.observer.take();
          self.pending.extend(events);
          self.done = true;
        }
      }
    }
  }

  fn schedule_recovery(&mut self, error: Option<Error>) -> Result<(), Error> {
    let fallback =
      || Error::Malformed("stream recovery has no checkpoint or exhausted its budget".into());
    let Some(r) = &mut self.recovery else {
      return Err(error.unwrap_or_else(fallback));
    };
    if r.token.is_none()
      || r.decoder.is_none()
      || r.hops >= 32
      || r.failures >= r.policy.max_attempts.saturating_sub(1)
      || error.as_ref().is_some_and(|e| !r.protocol.permits(e))
    {
      return Err(error.unwrap_or_else(fallback));
    }
    self.observer.take();
    self.records.take();
    r.hops += 1;
    let delay = if r.continuation {
      0
    } else {
      r.failures += 1;
      r.policy.calculate_backoff_ms(r.failures, error.as_ref().and_then(Error::get_retry_after_ms))
    };
    r.continuation = false;
    r.waiting = Some(delay);
    self.pending.push_back(StreamEvent::Rewind { revision: r.revision });
    self.pending.push_back(StreamEvent::Recovering { attempt: r.hops, delay_ms: delay });
    // Publish the checkpoint's usage after the rewind, including to call statistics.
    self.pending.push_back(StreamEvent::Usage(crate::protocol::model_use::recovery::sum_usage(
      r.previous_usage,
      r.committed_usage,
    )));
    Ok(())
  }

  async fn reopen(&mut self) -> Result<(), Error> {
    let r = self.recovery.as_mut().expect("scheduled recovery");
    let delay = r.waiting.take().expect("scheduled wait");
    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
    let call = r.protocol.resume(&r.original, r.token.as_deref().expect("checkpoint"))?;
    self.observer = r.observer.as_ref().map(|factory| factory(&r.model));
    let result = async {
      let mut reply = self.transport.execute_stream(&call).await?;
      if !reply.is_success() {
        let body = reply.read_error_body().await;
        return Err(
          crate::protocol::http_error::decode_error_reply(
            reply.get_status(),
            &body,
            |status, body| self.protocol.decode_http_error(status, body),
          )
          .with_retry_after(reply.get_retry_after_ms()),
        );
      }
      Ok(reply)
    }
    .await;
    match result {
      Ok(reply) => {
        self.records = Some(RecordStream::new(reply, crate::transport::Framing::Sse));
        self.decoder =
          r.decoder.as_ref().and_then(StreamDecoder::checkpoint).expect("checkpoint decoder");
        r.previous_usage =
          crate::protocol::model_use::recovery::sum_usage(r.previous_usage, r.committed_usage);
        r.terminal = false;
        r.committed_usage = Default::default();
        r.usage = Default::default();
        Ok(())
      }
      Err(error) => self.schedule_recovery(Some(error)),
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

impl<T: Transport + Send + Sync> ModelStream for EventStream<T> {
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
