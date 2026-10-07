//! One-second observations of received output, separate from provider-reported billing usage.
use crate::server::{management::ManagementStore, provider::ModelClient};
use crate::{client::AttemptObserver, protocol::StreamEvent, utils::time::Timestamp};
use std::sync::{
  Arc,
  atomic::{AtomicU64, Ordering},
};
use tokio::{
  sync::oneshot,
  time::{Duration, Instant, MissedTickBehavior},
};
use tokio_util::task::TaskTracker;

/// How often a stream is sampled.
pub const INTERVAL: Duration = Duration::from_secs(1);
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
struct Sampler {
  bytes: Arc<AtomicU64>,
  finish: Option<oneshot::Sender<Instant>>,
}
impl AttemptObserver for Sampler {
  fn observe(&self, event: &StreamEvent) {
    let text = match event {
      StreamEvent::TextDelta { delta, .. } | StreamEvent::ReasoningDelta { delta, .. } => delta,
      StreamEvent::ToolUseDelta { arguments, .. } => arguments,
      _ => return,
    };
    self.bytes.fetch_add(text.len() as u64, Ordering::Relaxed);
  }
}
impl Drop for Sampler {
  fn drop(&mut self) {
    if let Some(finish) = self.finish.take() {
      let _ = finish.send(Instant::now());
    }
  }
}
/// The client with every stream it opens sampled into the index, each interval's received bytes
/// as one sample.
pub fn with_stream_sampling(
  client: &ModelClient,
  management: Arc<ManagementStore>,
  tasks: TaskTracker,
  provider: String,
  session: Option<String>,
) -> ModelClient {
  client.clone().with_attempt_observer(Arc::new(move |model| {
    let bytes = Arc::new(AtomicU64::new(0));
    let (finish, mut finished) = oneshot::channel();
    let start = Instant::now();
    let at_ms = Timestamp::now().0;
    let mut sample = Sample {
      attempt_id: uuid::Uuid::new_v4().to_string(),
      session: session.clone(),
      provider: provider.clone(),
      model: model.into(),
      at_ms,
      duration_ms: 0,
      output_bytes: 0,
    };
    let (counter, management) = (bytes.clone(), management.clone());
    tasks.spawn(async move {
      let mut timer = tokio::time::interval_at(start + INTERVAL, INTERVAL);
      timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
      let (mut previous_time, mut previous_bytes) = (start, 0);
      loop {
        let (now, terminal) = tokio::select! {
          biased;
          ended = &mut finished => (ended.unwrap_or_else(|_| Instant::now()), true),
          _ = timer.tick() => (Instant::now(), false),
        };
        let total = counter.load(Ordering::Relaxed);
        let duration_ms = now.duration_since(previous_time).as_millis() as u64;
        if duration_ms > 0 {
          sample.at_ms = at_ms + now.duration_since(start).as_millis() as u64;
          sample.duration_ms = duration_ms;
          sample.output_bytes = total - previous_bytes;
          let (management, sample) = (management.clone(), sample.clone());
          match tokio::task::spawn_blocking(move || management.save_stream_sample(&sample)).await {
            Ok(Ok(())) => {}
            result => {
              eprintln!("stream sample persistence failed: {result:?}");
              break;
            }
          }
        }
        previous_time = now;
        previous_bytes = total;
        if terminal {
          break;
        }
      }
      // A call's samples may take the index past its limit.
      let merged = tokio::task::spawn_blocking(move || management.merge_stream_samples()).await;
      if !matches!(merged, Ok(Ok(_))) {
        eprintln!("merging stream samples failed: {merged:?}");
      }
    });
    Box::new(Sampler { bytes, finish: Some(finish) })
  }))
}
