use crate::{
  Error,
  executor::model::{CallResponse, Client},
  protocol::{Request, StreamAccumulator, StreamEvent, wire::Transport},
};
use std::future::Future;

/// Executes model requests; the target model is selected by Request::model.
/// Returns a complete response or a stream according to Request::stream.
/// Retries remain in the client and end before it hands out the first event.
pub trait ModelCaller: Sync {
  /// Capability is explicit; configured compaction errors must not fall back to local summaries.
  fn supports_upstream_compaction(&self) -> bool {
    false
  }
  fn compact_upstream(
    &self,
    _request: &crate::protocol::upstream_compaction::UpstreamCompactionRequest,
  ) -> impl Future<Output = Result<crate::protocol::upstream_compaction::UpstreamCompaction, Error>> + Send
  {
    async { Err(Error::Build("upstream compaction is not configured".into())) }
  }
  /// None means no count endpoint is configured. Configured endpoint failures remain errors.
  fn count_tokens(
    &self,
    _request: &Request,
  ) -> impl Future<Output = Result<Option<crate::protocol::TokenCount>, Error>> + Send {
    async { Ok(None) }
  }
  /// Validate a replacement context with the same renderer used for actual model calls.
  fn validate_request(&self, request: &Request) -> Result<(), Error> {
    crate::protocol::model_use::context::find_boundaries(&request.conversation)?;
    match self.get_model_use_protocol() {
      Some(protocol) => protocol.render(request).map(|_| ()),
      None => {
        Err(Error::Build("compaction requires a protocol or a custom request validator".into()))
      }
    }
  }
  /// Supplies reasoning replay semantics for buffered output-limit responses.
  fn get_model_use_protocol(&self) -> Option<crate::protocol::model_use::ModelUseProtocol> {
    None
  }
  type Stream: ModelStream;
  fn call(
    &self,
    request: &Request,
  ) -> impl Future<Output = Result<CallResponse<Self::Stream>, Error>> + Send;
}

/// Dropping/aborting a stream abandons local reads without flushing an incomplete wire frame.
/// A pending `next` future need not be resumable after cancellation: the agent then aborts the
/// entire stream. Custom implementations own cleanup of any background work they start.
pub trait ModelStream: Send {
  fn create_accumulator(&self) -> StreamAccumulator {
    StreamAccumulator::new()
  }
  fn next(&mut self) -> impl Future<Output = Result<Option<StreamEvent>, Error>> + Send;
  fn abort(self)
  where
    Self: Sized,
  {
    drop(self);
  }
}

impl<T: Transport + Sync> ModelCaller for Client<T> {
  fn supports_upstream_compaction(&self) -> bool {
    self.get_upstream_compaction_protocol().is_some()
  }
  async fn compact_upstream(
    &self,
    request: &crate::protocol::upstream_compaction::UpstreamCompactionRequest,
  ) -> Result<crate::protocol::upstream_compaction::UpstreamCompaction, Error> {
    Client::compact_upstream(self, request).await
  }

  async fn count_tokens(
    &self,
    request: &Request,
  ) -> Result<Option<crate::protocol::TokenCount>, Error> {
    if self.get_token_count_protocol().is_some() {
      Client::count_tokens(self, request).await.map(Some)
    } else {
      Ok(None)
    }
  }
  fn get_model_use_protocol(&self) -> Option<crate::protocol::model_use::ModelUseProtocol> {
    Some(Client::get_model_use_protocol(self))
  }
  type Stream = crate::executor::model::client::EventStream<T::Stream>;
  async fn call(&self, request: &Request) -> Result<CallResponse<Self::Stream>, Error> {
    Client::call(self, request).await
  }
}

impl<S: crate::protocol::wire::ReplyStream> ModelStream
  for crate::executor::model::client::EventStream<S>
{
  fn create_accumulator(&self) -> StreamAccumulator {
    self.create_accumulator()
  }
  async fn next(&mut self) -> Result<Option<StreamEvent>, Error> {
    self.next().await
  }
  fn abort(self) {
    self.abort();
  }
}
