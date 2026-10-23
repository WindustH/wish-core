//! Retry progress belongs to one logical upstream request, including its cancellation.
use crate::Error;
use crate::utils::{
  retry::{RetryDecision, RetryEvent, RetryPolicy, retry_observed},
  time::Timestamp,
};
use std::{future::Future, sync::Arc};

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryOperation {
  Model,
  Compaction,
  TokenCount,
  ModelList,
  AccountState,
  WebSearch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryPhase {
  Waiting,
  Attempting,
  Finished,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RetryStatus {
  pub request_id: String,
  pub operation: RetryOperation,
  pub phase: RetryPhase,
  pub retry: u32,
  pub max_retries: u32,
  pub delay_ms: u64,
  /// Server wall-clock timestamps let reconnecting clients recover the remaining wait.
  pub observed_at: u64,
  pub retry_at: u64,
  pub error: Error,
}

pub type RetryObserver = Arc<dyn Fn(&RetryStatus) + Send + Sync>;

struct Progress {
  observer: Option<RetryObserver>,
  operation: RetryOperation,
  status: Option<RetryStatus>,
}
impl Progress {
  fn observe(&mut self, event: RetryEvent<'_, Error>) {
    if self.observer.is_none() {
      return;
    }
    match event {
      RetryEvent::Waiting { retry, max_retries, delay_ms, error } => {
        let now = Timestamp::now().0;
        self.status = Some(RetryStatus {
          request_id: self
            .status
            .as_ref()
            .map(|s| s.request_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
          operation: self.operation,
          phase: RetryPhase::Waiting,
          retry,
          max_retries,
          delay_ms,
          observed_at: now,
          retry_at: now.saturating_add(delay_ms),
          error: error.clone(),
        });
      }
      RetryEvent::Attempting { retry } => {
        if let Some(status) = &mut self.status {
          status.phase = RetryPhase::Attempting;
          status.retry = retry;
          status.observed_at = Timestamp::now().0;
        }
      }
    }
    if let (Some(observer), Some(status)) = (&self.observer, &self.status) {
      observer(status);
    }
  }
}
impl Drop for Progress {
  fn drop(&mut self) {
    // Also runs when an interrupt/shutdown drops a sleeping or in-flight request.
    if let (Some(observer), Some(status)) = (&self.observer, &mut self.status) {
      status.phase = RetryPhase::Finished;
      status.observed_at = Timestamp::now().0;
      observer(status);
    }
  }
}

pub(crate) async fn retry_upstream<T, F, Fut>(
  policy: &RetryPolicy,
  operation: RetryOperation,
  observer: Option<RetryObserver>,
  attempt: F,
  classify: fn(&Error) -> RetryDecision,
) -> Result<T, Error>
where
  F: FnMut(u32) -> Fut,
  Fut: Future<Output = Result<T, Error>>,
{
  let mut progress = Progress { observer, operation, status: None };
  retry_observed(policy, attempt, classify, |event| progress.observe(event)).await
}

/// Read-only auxiliary operations may repeat even when the connection failed after sending.
pub(crate) fn classify_read_retry(error: &Error) -> RetryDecision {
  if matches!(error, Error::Transport(_)) || error.is_retryable() {
    RetryDecision::Retry { minimum_delay_ms: error.get_retry_after_ms() }
  } else {
    RetryDecision::Stop
  }
}
