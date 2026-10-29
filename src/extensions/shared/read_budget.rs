//! Shared admission and wall-clock budget for auxiliary read operations.
use std::time::Duration;
static PERMITS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(16);
pub(crate) async fn run<T>(operation: impl Future<Output = T>) -> Result<T, String> {
  tokio::time::timeout(Duration::from_secs(120), async {
    let _permit = PERMITS.acquire().await.map_err(|_| "web request queue closed".to_owned())?;
    Ok(operation.await)
  })
  .await
  .map_err(|_| "web request exceeded the 120 second total budget".to_owned())?
}
