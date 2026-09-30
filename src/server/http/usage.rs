//! Usage statistics: SQL aggregates over the model calls the index holds, all sessions' or one's,
//! and the stream samples beside them. Conversation payloads are never read.
use crate::server::{
  app::App,
  error::{ApiError, blocking},
  management::{StreamAggregate, StreamSample, Usage, UsageBucket},
  sampling,
};
use axum::{
  Json,
  extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

fn now() -> i64 {
  crate::utils::time::Timestamp::now().0 as i64
}
/// Refuses a session the index does not know.
async fn require_indexed(app: &App, id: &str) -> Result<(), ApiError> {
  let (management, id) = (app.management.clone(), id.to_owned());
  if blocking(move || management.exists(&id)).await? { Ok(()) } else { Err(ApiError::not_found()) }
}
fn describe_totals(usage: &Usage) -> Value {
  json!({
    "usage_records": usage.with_usage,
    "committed_responses": usage.completed,
    "tokens": {
      "input_tokens": usage.input_tokens,
      "output_tokens": usage.output_tokens,
      "total_tokens": usage.total_tokens,
      "reasoning_tokens": usage.reasoning_tokens,
    },
    "cache": {
      "read_input_tokens": usage.cached_input_tokens,
      "write_input_tokens": usage.cache_write_input_tokens,
      "request_hit_ratio": null,
    },
  })
}
/// The start of every bucket of `step` from `from` up to `to`.
fn bucket_starts(from: i64, to: i64, step: i64) -> impl Iterator<Item = i64> {
  (0..).map(move |n| from + n * step).take_while(move |at| *at < to)
}

/// Optional window for the totals; all recorded usage when absent.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
  from_ms: Option<i64>,
  to_ms: Option<i64>,
}
async fn usage(
  app: Arc<App>,
  session: Option<String>,
  range: Range,
) -> Result<Json<Value>, ApiError> {
  let from = range.from_ms.unwrap_or(0);
  let to = range.to_ms.unwrap_or_else(|| now() + 1);
  if from < 0 || to <= from {
    return Err(ApiError::bad_request("invalid usage range"));
  }
  let management = app.management.clone();
  let rows =
    blocking(move || management.usage_buckets(session.as_deref(), from, to, i64::MAX)).await?;
  let total: Usage = rows.iter().sum();
  let by_provider_model: Vec<_> = rows
    .iter()
    .map(
      |row| json!({"provider":row.provider,"model":row.model,"totals":describe_totals(&row.usage)}),
    )
    .collect();
  Ok(Json(json!({
    "unit": "logical_model_call",
    "statistics": {
      "model_attempts": total.attempts,
      "attempts_with_usage": total.with_usage,
      "attempts_without_usage": total.attempts - total.with_usage,
      "totals": describe_totals(&total),
      "by_provider_model": by_provider_model,
    },
  })))
}
pub async fn global_usage(
  State(app): State<Arc<App>>,
  Query(range): Query<Range>,
) -> Result<Json<Value>, ApiError> {
  usage(app, None, range).await
}
pub async fn session_usage(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(range): Query<Range>,
) -> Result<Json<Value>, ApiError> {
  require_indexed(&app, &id).await?;
  usage(app, Some(id), range).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Series {
  window: String,
  bucket: Option<String>,
  from_ms: Option<i64>,
  to_ms: Option<i64>,
}
/// One provider and model's part of a series.
#[derive(Default)]
struct Group {
  buckets: Vec<UsageBucket>,
  samples: Vec<StreamSample>,
  aggregates: Vec<StreamAggregate>,
}
/// Stream samples summed: estimated tokens, the time they took, and how many.
#[derive(Clone, Copy, Default)]
struct Rate {
  tokens: f64,
  duration_ms: i64,
  samples: i64,
}
impl Rate {
  fn add(&mut self, aggregate: &StreamAggregate) {
    self.tokens += aggregate.output_tokens;
    self.duration_ms += aggregate.duration_ms;
    self.samples += aggregate.count;
  }
  /// Tokens a second, when any time was measured.
  fn per_second(&self) -> Option<f64> {
    (self.duration_ms > 0).then(|| self.tokens * 1000.0 / self.duration_ms as f64)
  }
}
async fn series(app: Arc<App>, id: Option<String>, query: Series) -> Result<Json<Value>, ApiError> {
  let to = query.to_ms.unwrap_or_else(now);
  let days = match query.window.as_str() {
    "1d" => 1,
    "7d" => 7,
    "30d" => 30,
    "90d" => 90,
    "365d" => 365,
    "custom" => 0,
    _ => return Err(ApiError::bad_request("unknown window")),
  };
  let from = if days == 0 {
    query.from_ms.ok_or_else(|| ApiError::bad_request("custom window requires from_ms"))?
  } else {
    to - days * DAY_MS
  };
  let bucket = query.bucket.unwrap_or_else(|| "1d".into());
  let step = match bucket.as_str() {
    "1h" => HOUR_MS,
    "6h" => 6 * HOUR_MS,
    "1d" => DAY_MS,
    _ => return Err(ApiError::bad_request("unknown bucket")),
  };
  if from < 0 || to <= from || to - from > 366 * DAY_MS {
    return Err(ApiError::bad_request("invalid time range"));
  }
  let management = app.management.clone();
  let (rows, samples, aggregates) = blocking(move || {
    let session = id.as_deref();
    Ok((
      management.usage_buckets(session, from, to, step)?,
      management.read_stream_samples(session, from, to)?,
      management.aggregate_stream_samples(session, from, to, step)?,
    ))
  })
  .await?;
  let displayed_samples = samples.len();
  let sample_count: i64 = aggregates.iter().map(|aggregate| aggregate.count).sum();
  let mut groups: BTreeMap<(String, String), Group> = BTreeMap::new();
  for row in rows {
    groups.entry((row.provider.clone(), row.model.clone())).or_default().buckets.push(row);
  }
  for sample in samples {
    groups.entry((sample.provider.clone(), sample.model.clone())).or_default().samples.push(sample);
  }
  for aggregate in aggregates {
    let key = (aggregate.provider.clone(), aggregate.model.clone());
    groups.entry(key).or_default().aggregates.push(aggregate);
  }
  let mut total = Usage::default();
  let mut output = Vec::new();
  for ((provider, model), mut group) in groups {
    group.samples.sort_by_key(|sample| sample.at_ms);
    let usage: Usage = group.buckets.iter().sum();
    total = total + usage;
    let by_start: BTreeMap<i64, &Usage> =
      group.buckets.iter().map(|bucket| (bucket.start_ms, &bucket.usage)).collect();
    let mut rates: BTreeMap<i64, Rate> = BTreeMap::new();
    let mut window = Rate::default();
    for aggregate in &group.aggregates {
      rates.entry(from + (aggregate.at_ms - from) / step * step).or_default().add(aggregate);
      window.add(aggregate);
    }
    let buckets: Vec<_> = bucket_starts(from, to, step)
      .map(|at| {
        let usage = by_start.get(&at).copied().copied().unwrap_or_default();
        let rate = rates.get(&at).copied().unwrap_or_default();
        json!({
          "start_ms": at,
          "input_tokens": usage.input_tokens,
          "output_tokens": usage.output_tokens,
          "total_tokens": usage.total_tokens,
          "attempts": usage.attempts,
          "tps": rate.per_second(),
          "stream_samples": rate.samples,
          "stream_output_tokens": rate.tokens,
          "stream_duration_ms": rate.duration_ms,
        })
      })
      .collect();
    output.push(json!({
      "provider": provider,
      "model": model,
      "samples": group.samples,
      "window": {"total_tokens": usage.total_tokens, "tps": window.per_second()},
      "buckets": buckets,
    }));
  }
  Ok(Json(json!({
    "unit": "logical_model_call",
    "sampling": {
      "interval_ms": sampling::INTERVAL.as_millis() as u64,
      "source": sampling::SOURCE,
      "bytes_per_token": sampling::BYTES_PER_TOKEN,
      "displayed_samples": displayed_samples,
      "sample_limit": sampling::LIST_LIMIT,
      "truncated": sample_count > displayed_samples as i64,
    },
    "query": {"from_ms": from, "to_ms": to, "window": query.window, "bucket": bucket, "bucket_ms": step},
    "coverage": {
      "attempts_with_usage": total.with_usage,
      "attempts_success": total.completed,
      "attempts_failed": total.attempts - total.completed,
      "stream_samples": sample_count,
    },
    "groups": output,
  })))
}
pub async fn global_series(
  State(app): State<Arc<App>>,
  Query(query): Query<Series>,
) -> Result<Json<Value>, ApiError> {
  series(app, None, query).await
}
pub async fn session_series(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<Series>,
) -> Result<Json<Value>, ApiError> {
  require_indexed(&app, &id).await?;
  series(app, Some(id), query).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Daily {
  days: i64,
  end_date: String,
  tz_offset_minutes: i64,
  bucket_ms: Option<i64>,
}
/// Dates read and written the way SQLite reads and writes them.
struct Calendar(rusqlite::Connection);
impl Calendar {
  fn open() -> Result<Self, ApiError> {
    Ok(Self(rusqlite::Connection::open_in_memory()?))
  }
  /// Unix milliseconds at the start of a UTC date, or none when it is not one.
  fn parse_day_start(&self, date: &str) -> Result<Option<i64>, ApiError> {
    Ok(self.0.query_row("SELECT unixepoch(?1)*1000", [date], |r| r.get(0))?)
  }
  /// The UTC date at unix milliseconds.
  fn format_date(&self, ms: i64) -> Result<String, ApiError> {
    Ok(self.0.query_row("SELECT date(?1/1000,'unixepoch')", [ms], |r| r.get(0))?)
  }
}
async fn daily(app: Arc<App>, id: Option<String>, query: Daily) -> Result<Json<Value>, ApiError> {
  if !(1..=366).contains(&query.days) || !(-840..=840).contains(&query.tz_offset_minutes) {
    return Err(ApiError::bad_request("invalid days or timezone"));
  }
  let management = app.management.clone();
  blocking(move || {
    let offset = query.tz_offset_minutes * 60_000;
    let calendar = Calendar::open()?;
    let end = calendar.parse_day_start(&query.end_date)?;
    let last = end.ok_or_else(|| ApiError::bad_request("invalid end_date"))? - offset;
    let first = last - (query.days - 1) * DAY_MS;
    let to = last + DAY_MS;
    let step = query.bucket_ms.unwrap_or(DAY_MS);
    if step <= 0 || (to - first) / step > 100_000 {
      return Err(ApiError::bad_request("invalid bucket_ms"));
    }
    let rows = management.usage_buckets(id.as_deref(), first, to, step)?;
    let by_day = management.usage_buckets(id.as_deref(), first, to, DAY_MS)?;
    let starting_at =
      |rows: &[UsageBucket], at: i64| -> Usage { rows.iter().filter(|r| r.start_ms == at).sum() };
    let buckets: Vec<_> = bucket_starts(first, to, step)
      .map(|at| json!({"start_ms":at,"total_tokens":starting_at(&rows, at).total_tokens}))
      .collect();
    let mut days = Vec::new();
    for at in bucket_starts(first, to, DAY_MS) {
      let usage = starting_at(&by_day, at);
      days.push(json!({
        "date": calendar.format_date(at + offset)?,
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "total_tokens": usage.total_tokens,
        "attempts": usage.attempts,
      }));
    }
    Ok(Json(json!({
      "query": {
        "days": query.days,
        "end_date": query.end_date,
        "tz_offset_minutes": query.tz_offset_minutes,
        "tz_label": format!("UTC{:+}", query.tz_offset_minutes as f64 / 60.0),
        "first_day_start_ms": first,
        "last_day_start_ms": last,
        "bucket_ms": step,
      },
      "days": days,
      "buckets": buckets,
    })))
  })
  .await
}
pub async fn global_daily(
  State(app): State<Arc<App>>,
  Query(query): Query<Daily>,
) -> Result<Json<Value>, ApiError> {
  daily(app, None, query).await
}
pub async fn session_daily(
  State(app): State<Arc<App>>,
  Path(id): Path<String>,
  Query(query): Query<Daily>,
) -> Result<Json<Value>, ApiError> {
  require_indexed(&app, &id).await?;
  daily(app, Some(id), query).await
}
