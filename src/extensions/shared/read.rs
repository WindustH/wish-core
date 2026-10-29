//! Per-attempt deadlines and retry policy shared by auxiliary read operations.
use crate::client::{
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

/// Read-only extension calls share retry mechanics, not a feature-specific request type.
pub struct ReadClient<T> {
  transport: T,
  credentials: crate::protocol::endpoint::Credentials,
  policy: RetryPolicy,
  observer: Option<RetryObserver>,
}
impl<T: crate::protocol::attempt::Transport> ReadClient<T> {
  pub fn new(
    transport: T,
    credentials: crate::protocol::endpoint::Credentials,
    policy: RetryPolicy,
    observer: Option<RetryObserver>,
  ) -> Self {
    Self { transport, credentials, policy, observer }
  }
  pub(crate) async fn execute<'a, R, F, Fut>(
    &'a self,
    operation: RetryOperation,
    mut attempt: F,
  ) -> Result<R, Error>
  where
    F: FnMut(&'a T, &'a crate::protocol::endpoint::Credentials) -> Fut,
    Fut: Future<Output = Result<R, Error>>,
  {
    read(&self.policy, operation, self.observer.clone(), || {
      attempt(&self.transport, &self.credentials)
    })
    .await
  }
}
