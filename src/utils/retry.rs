//! How many times one call may be attempted, and how long to wait between attempts.
//!
//! The policy is pure data plus one arithmetic step, and `retry` is the loop that spends it.
//! What one attempt *is* stays the caller's business: the client's buffered attempt is the whole
//! call, its streamed one the opening and the first event, so a stream is replaced only while
//! nothing has been handed over yet.

use std::future::Future;
use std::time::Duration;

/// Bounded attempt policy for one logical call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RetryPolicy {
  /// Max attempts per logical operation (including the first). `1` disables retry.
  pub max_attempts: u32,
  /// Base wait for the first retry, in milliseconds.
  pub delay_ms: u64,
  /// Additive random wait in [0, jitter_ms), never subtracted from the base wait.
  pub jitter_ms: u64,
  /// Grow both waits linearly with the retry number; otherwise keep them fixed.
  pub linear: bool,
}

impl Default for RetryPolicy {
  fn default() -> Self {
    Self::auxiliary()
  }
}

impl RetryPolicy {
  /// Five retries after the original call: 5s + up to 1s, 10s + up to 2s, ... 25s + up to 5s.
  pub const fn model() -> Self {
    Self { max_attempts: 6, delay_ms: 5_000, jitter_ms: 1_000, linear: true }
  }

  /// Three retries after the original read/search, each waiting 2s + up to 1s.
  pub const fn auxiliary() -> Self {
    Self { max_attempts: 4, delay_ms: 2_000, jitter_ms: 1_000, linear: false }
  }

  /// Additive jitter, honoring `Retry-After` as a minimum even when it exceeds our own wait.
  #[must_use]
  pub fn calculate_backoff_ms(&self, attempt_no: u32, retry_after_ms: Option<u64>) -> u64 {
    let step = if self.linear { u64::from(attempt_no.max(1)) } else { 1 };
    let base = self.delay_ms.saturating_mul(step);
    let jitter = (self.jitter_ms.saturating_mul(step) as f64 * generate_random_unit()) as u64;
    base.saturating_add(jitter).max(retry_after_ms.unwrap_or(0))
  }
}

/// The caller decides whether repeating its operation is safe and useful.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryDecision {
  Stop,
  Retry { minimum_delay_ms: Option<u64> },
}

pub enum RetryEvent<'a, E> {
  Waiting { retry: u32, max_retries: u32, delay_ms: u64, error: &'a E },
  Attempting { retry: u32 },
}

/// Retry an operation with caller-owned error classification. Attempt numbers start at one.
/// Even a zero max_attempts value permits the initial attempt, but no retries.
/// Dropping this future cancels local waiting; it cannot undo an operation's external effects.
#[allow(dead_code)] // The policy-only entry point is also used by embedded callers.
pub async fn retry<T, E, F, Fut, Decide>(
  policy: &RetryPolicy,
  attempt: F,
  decide: Decide,
) -> Result<T, E>
where
  F: FnMut(u32) -> Fut,
  Fut: Future<Output = Result<T, E>>,
  Decide: FnMut(&E) -> RetryDecision,
{
  retry_observed(policy, attempt, decide, |_| {}).await
}

/// Observes the wait before, and start of, each retry. Dropping the future cancels the wait.
pub async fn retry_observed<T, E, F, Fut, Decide, Observe>(
  policy: &RetryPolicy,
  mut attempt: F,
  mut decide: Decide,
  mut observe: Observe,
) -> Result<T, E>
where
  F: FnMut(u32) -> Fut,
  Fut: Future<Output = Result<T, E>>,
  Decide: FnMut(&E) -> RetryDecision,
  Observe: FnMut(RetryEvent<'_, E>),
{
  let mut number = 1;
  loop {
    match attempt(number).await {
      Ok(value) => return Ok(value),
      Err(error) => {
        if number >= policy.max_attempts {
          return Err(error);
        }
        let RetryDecision::Retry { minimum_delay_ms } = decide(&error) else {
          return Err(error);
        };
        let delay = policy.calculate_backoff_ms(number, minimum_delay_ms);
        observe(RetryEvent::Waiting {
          retry: number,
          max_retries: policy.max_attempts.saturating_sub(1),
          delay_ms: delay,
          error: &error,
        });
        tokio::time::sleep(Duration::from_millis(delay)).await;
        observe(RetryEvent::Attempting { retry: number });
        number += 1;
      }
    }
  }
}

/// Deterministic-enough unit sample in [0, 1): splitmix over the clock and the process id, which
/// keeps a jitter dependency out of the crate.
fn generate_random_unit() -> f64 {
  let nanos = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map_or(0, |duration| u64::from(duration.subsec_nanos()) ^ duration.as_secs());
  let mixed = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(u64::from(std::process::id()));
  let shifted = mixed ^ (mixed >> 30);
  ((shifted % (1u64 << 53)) as f64) / ((1u64 << 53) as f64)
}
