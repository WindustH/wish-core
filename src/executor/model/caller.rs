//! What the executor asks of a model provider: calls, and the optional counting, compaction and
//! validation that context compaction uses.
use crate::{
  Error,
  protocol::{
    Request, Response, StreamAccumulator, StreamEvent, TokenCount,
    model_use::{ModelUseProtocol, context::find_replay_unit_boundaries},
    upstream_compaction::{UpstreamCompaction, UpstreamCompactionRequest},
  },
};
use std::future::Future;

/// The response mode selected by `Request::stream`.
pub enum CallResponse<S> {
  Complete(Box<Response>),
  Stream(S),
}

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
    _request: &UpstreamCompactionRequest,
  ) -> impl Future<Output = Result<UpstreamCompaction, Error>> + Send {
    async { Err(Error::Build("upstream compaction is not configured".into())) }
  }
  /// None means no count endpoint is configured. Configured endpoint failures remain errors.
  fn count_tokens(
    &self,
    _request: &Request,
  ) -> impl Future<Output = Result<Option<TokenCount>, Error>> + Send {
    async { Ok(None) }
  }
  /// Validate a replacement context with the same renderer used for actual model calls.
  fn validate_request(&self, request: &Request) -> Result<(), Error> {
    find_replay_unit_boundaries(&request.conversation)?;
    match self.get_model_use_protocol() {
      Some(protocol) => protocol.render(request).map(|_| ()),
      None => {
        Err(Error::Build("compaction requires a protocol or a custom request validator".into()))
      }
    }
  }
  /// Supplies reasoning replay semantics for buffered output-limit responses.
  fn get_model_use_protocol(&self) -> Option<ModelUseProtocol> {
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
