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

#[cfg(test)]
mod tests {
  use super::*;
  #[tokio::test]
  async fn cancellation_keeps_running_work_admitted_and_drops_waiters() {
    let capacity = Arc::new(Semaphore::new(1));
    let (started, start) = tokio::sync::oneshot::channel();
    let (release, finish) = std::sync::mpsc::channel();
    let running = tokio::spawn(run(capacity.clone(), move || {
      started.send(()).unwrap();
      finish.recv().unwrap();
    }));
    start.await.unwrap();
    running.abort();
    assert_eq!(capacity.available_permits(), 0);
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let marker = called.clone();
    let waiting = tokio::spawn(run(capacity.clone(), move || {
      marker.store(true, std::sync::atomic::Ordering::SeqCst);
    }));
    tokio::task::yield_now().await;
    waiting.abort();
    let _ = waiting.await;
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), run(capacity.clone(), || ()))
      .await
      .unwrap()
      .unwrap();
    assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(capacity.available_permits(), 1);
  }
  #[tokio::test]
  async fn burst_respects_admission_limit() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let capacity = Arc::new(Semaphore::new(3));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..48 {
      let (capacity, active, peak) = (capacity.clone(), active.clone(), peak.clone());
      tasks.spawn(run(capacity, move || {
        let running = active.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(running, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(2));
        active.fetch_sub(1, Ordering::SeqCst);
      }));
    }
    while let Some(result) = tasks.join_next().await {
      result.unwrap().unwrap();
    }
    assert!(peak.load(Ordering::SeqCst) <= 3);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(capacity.available_permits(), 3);
  }
  #[tokio::test]
  async fn panics_release_capacity() {
    let capacity = Arc::new(Semaphore::new(1));
    assert!(run(capacity.clone(), || panic!("test worker panic")).await.unwrap_err().is_panic());
    assert_eq!(run(capacity, || 42).await.unwrap(), 42);
  }
}
