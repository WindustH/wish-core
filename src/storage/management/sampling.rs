//! Persisted stream-sample representation and its measurement units.
/// Received bytes counted as one token: the ratio of core's default `TokenEstimator`, without its
/// chunk rounding.
pub const BYTES_PER_TOKEN: u64 = 4;
/// What the samples measure, as the usage series names it.
pub const SOURCE: &str = "estimated_visible_output";
/// The most samples a usage series lists.
pub const LIST_LIMIT: usize = 10_000;

/// One interval of one model call's received output, as the index stores it.
#[derive(Clone)]
pub struct Sample {
  pub attempt_id: String,
  pub session: Option<String>,
  pub provider: String,
  pub model: String,
  pub at_ms: u64,
  pub duration_ms: u64,
  pub output_bytes: u64,
}
