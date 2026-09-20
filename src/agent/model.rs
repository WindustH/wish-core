use crate::{
  Error,
  client::{CallResponse, Client},
  protocol::{Request, StreamAccumulator, StreamEvent, wire::Transport},
};
use std::future::Future;

/// A model executes the response mode declared by Request::stream.
/// Retries remain in the client and end before it hands out the first event.
pub trait Model: Sync {
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

impl<T: Transport + Sync> Model for Client<T> {
  type Stream = crate::client::EventStream<T::Stream>;
  async fn call(&self, request: &Request) -> Result<CallResponse<Self::Stream>, Error> {
    Client::call(self, request).await
  }
}

impl<S: crate::protocol::wire::ReplyStream> ModelStream for crate::client::EventStream<S> {
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
