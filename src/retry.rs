//! How many times one call may be attempted, and how long to wait between attempts.
//!
//! The policy is pure data plus one arithmetic step, and `retry` is the loop that spends it.
//! What one attempt *is* stays the caller's business: a buffered call replaces it whole, a streamed
//! one only while nothing has been handed over yet, which is why the client keeps its own loop for
//! the second case.

use std::future::Future;
use std::time::Duration;

use crate::protocol::error::Error;

/// Bounded attempt policy for one logical call.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RetryPolicy {
  /// Max attempts per logical operation (including the first). `1` disables retry.
  pub max_attempts: u32,
  /// Initial backoff delay (ms).
  pub initial_delay_ms: u64,
  /// Max backoff delay (ms).
  pub max_delay_ms: u64,
  /// Backoff multiplier.
  pub multiplier: f64,
  /// Jitter ratio (0..=1).
  pub jitter_ratio: f64,
}

impl Default for RetryPolicy {
  fn default() -> Self {
    Self {
      max_attempts: 3,
      initial_delay_ms: 500,
      max_delay_ms: 30_000,
      multiplier: 2.0,
      jitter_ratio: 1.0,
    }
  }
}

impl RetryPolicy {
  /// Full-jitter backoff for the given attempt number (1-based), honoring an upstream `Retry-After`
  /// when one is present.
  ///
  /// The delay grows as `initial * multiplier^(attempt - 1)` and is capped at
  /// [`max_delay_ms`](Self::max_delay_ms), then spread uniformly over
  /// `[delay * (1 - jitter), delay]`. A `Retry-After` is a floor rather than a ceiling: when the
  /// upstream asks for a longer pause, the longer pause wins.
  #[must_use]
  pub fn calculate_backoff_ms(&self, attempt_no: u32, retry_after_ms: Option<u64>) -> u64 {
    let exp = f64::from(attempt_no.saturating_sub(1));
    let base = (self.initial_delay_ms as f64) * self.multiplier.powf(exp);
    let delay = base.min(self.max_delay_ms as f64) as u64;
    let retry_floor = retry_after_ms.unwrap_or(0);
    if self.jitter_ratio <= 0.0 {
      return delay.max(retry_floor);
    }
    // Full jitter: uniform in [delay * (1 - j), delay].
    let span = (delay as f64) * self.jitter_ratio;
    let floor = delay as f64 - span;
    let picked = floor + generate_random_unit() * span;
    (picked.min(self.max_delay_ms as f64) as u64).max(retry_floor)
  }
}

/// Runs one operation under a policy: the attempt is replaced while the failure looks transient and
/// attempts are left.
///
/// One attempt is whatever the caller hands over, which is what lets a buffered read and a streamed
/// one share the policy without sharing a definition of "delivered nothing yet".
pub(crate) async fn retry<T, F, Fut>(policy: &RetryPolicy, mut attempt: F) -> Result<T, Error>
where
  F: FnMut(u32) -> Fut,
  Fut: Future<Output = Result<T, Error>>,
{
  let mut number = 1;
  loop {
    match attempt(number).await {
      Ok(value) => return Ok(value),
      Err(error) => {
        if number >= policy.max_attempts || !error.is_retryable() {
          return Err(error);
        }
        let delay = policy.calculate_backoff_ms(number, error.get_retry_after_ms());
        tokio::time::sleep(Duration::from_millis(delay)).await;
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
