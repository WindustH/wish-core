//! Bounded retries for known status/result queries, never paid generation submissions.
use crate::{
  protocol::attempt::{Call, Reply, Transport},
  transport::ReqwestTransport,
};
use std::time::Duration;

pub(crate) async fn execute(transport: &ReqwestTransport, call: &Call) -> Result<Reply, String> {
  // Some providers query with POST, so safety is established by the caller's
  // decoded Pending state, not inferred from the HTTP method.
  for attempt in 0..3 {
    match transport.execute(call).await {
      Ok(reply) => {
        if !matches!(reply.status, 408 | 429 | 500 | 502 | 503 | 504) || attempt == 2 {
          return Ok(reply);
        }
        let delay = reply.get_header("retry-after")
          .and_then(|v| v.parse::<u64>().ok())
          .map(|seconds| seconds.clamp(2, 30))
          .unwrap_or(2 << attempt);
        tokio::time::sleep(Duration::from_secs(delay)).await;
      }
      Err(_) if attempt < 2 => {
        tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
      }
      Err(_) => return Err("Task query connection failed after bounded retries; the upstream task may still complete. Do not resubmit generation.".into()),
    }
  }
  unreachable!("bounded query loop always returns on its final attempt")
}
