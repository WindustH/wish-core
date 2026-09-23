//! One-second observations of received output, separate from provider-reported billing usage.
use crate::server::{management::ManagementStore, provider::ModelClient};
use serde::Serialize;
use std::sync::{
  Arc,
  atomic::{AtomicU64, Ordering},
};
use tokio::{
  sync::oneshot,
  time::{Duration, Instant, MissedTickBehavior},
};
use tokio_util::task::TaskTracker;
use crate::{
  executor::model::StreamObserver, protocol::StreamEvent, session::statistics::Timestamp,
};

#[derive(Clone, Serialize)]
pub struct Sample {
  pub attempt_id: String,
  pub session: Option<String>,
  pub provider: String,
  pub model: String,
  pub at_ms: u64,
  pub duration_ms: u64,
  pub output_bytes: u64,
  pub output_tokens: f64,
  pub tps: f64,
  pub source: &'static str,
}
struct Sampler {
  bytes: Arc<AtomicU64>,
  finish: Option<oneshot::Sender<Instant>>,
}
impl StreamObserver for Sampler {
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
pub fn observe_client(
  client: &ModelClient,
  index: Arc<ManagementStore>,
  tasks: TaskTracker,
  provider: String,
  session: Option<String>,
) -> ModelClient {
  client.clone().with_stream_observer(Arc::new(move |model| {
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
      output_tokens: 0.0,
      tps: 0.0,
      source: "estimated_visible_output",
    };
    let (counter, index) = (bytes.clone(), index.clone());
    tasks.spawn(async move {
      let mut timer =
        tokio::time::interval_at(start + Duration::from_secs(1), Duration::from_secs(1));
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
          // Same byte-ratio estimate as core's default TokenEstimator, without chunk rounding.
          sample.output_tokens = sample.output_bytes as f64 / 4.0;
          sample.tps = sample.output_tokens * 1000.0 / duration_ms as f64;
          let (index, sample) = (index.clone(), sample.clone());
          match tokio::task::spawn_blocking(move || index.save_stream_sample(&sample)).await {
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
    });
    Box::new(Sampler { bytes, finish: Some(finish) })
  }))
}
