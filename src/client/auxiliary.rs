//! Per-attempt deadlines and retry policy shared by auxiliary read operations.
use super::{
  RetryObserver, RetryOperation,
  retry::{classify_read_retry, retry_upstream},
};
use crate::{
  protocol::error::{Error, TransportFailure},
  utils::retry::RetryPolicy,
};
pub(super) async fn read<T, F, Fut>(
  policy: &RetryPolicy,
  operation: RetryOperation,
  observer: Option<RetryObserver>,
  mut attempt: F,
) -> Result<T, Error>
where
  F: FnMut() -> Fut,
  Fut: Future<Output = Result<T, Error>>,
{
  retry_upstream(
    policy,
    operation,
    observer,
    |_| {
      let future = attempt();
      async move {
        tokio::time::timeout(std::time::Duration::from_secs(60), future).await.unwrap_or_else(
          |_| {
            Err(Error::Transport(TransportFailure {
              retryable: false,
              message: "web request: no answer within 60 seconds".into(),
            }))
          },
        )
      }
    },
    classify_read_retry,
  )
  .await
}
