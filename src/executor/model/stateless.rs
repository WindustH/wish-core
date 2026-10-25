//! One model operation outside the session state machine; no history or tool execution.
use super::{CallResponse, ModelCaller, ModelStream};
use crate::{
  Error,
  protocol::{Request, Response},
};
pub async fn call_once(caller: &impl ModelCaller, request: &Request) -> Result<Response, Error> {
  match caller.call(request).await? {
    CallResponse::Complete(response) => Ok(*response),
    CallResponse::Stream(mut stream) => {
      let mut accumulator = stream.create_accumulator();
      while let Some(event) = stream.next().await? {
        accumulator.feed(event)?;
      }
      accumulator.finish()
    }
  }
}
