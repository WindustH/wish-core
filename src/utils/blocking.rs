//! Admission control for blocking work. Waiters stay asynchronous instead of occupying
//! Tokio blocking threads while SQLite or CPU work is already saturated.
use std::sync::{Arc, LazyLock};
use tokio::{sync::Semaphore, task::JoinError};

// Database and filesystem work can overlap, but SQLite writes remain serialized.
static IO: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(16)));
// Keep expensive diffs from occupying the entire blocking pool alongside storage work.
static CPU: LazyLock<Arc<Semaphore>> = LazyLock::new(|| {
  let processors = std::thread::available_parallelism().map_or(1, usize::from);
  Arc::new(Semaphore::new(processors.saturating_sub(1).clamp(1, 8)))
});

pub async fn io<F, R>(work: F) -> Result<R, JoinError>
where
  F: FnOnce() -> R + Send + 'static,
  R: Send + 'static,
{
  run(IO.clone(), work).await
}
pub async fn cpu<F, R>(work: F) -> Result<R, JoinError>
where
  F: FnOnce() -> R + Send + 'static,
  R: Send + 'static,
{
  run(CPU.clone(), work).await
}
async fn run<F, R>(capacity: Arc<Semaphore>, work: F) -> Result<R, JoinError>
where
  F: FnOnce() -> R + Send + 'static,
  R: Send + 'static,
{
  let permit = capacity.acquire_owned().await.expect("blocking admission stays open");
  tokio::task::spawn_blocking(move || {
    // The worker owns admission even if its async caller is cancelled. Started blocking
    // work cannot be aborted; releasing early would let replacements exceed the limit.
    let _permit = permit;
    work()
  })
  .await
}
