//! SQL aggregates over logical call observations, never conversation payloads.
use crate::server::{
  app::App,
  error::{ApiError, blocking},
};
use axum::{
  Json,
  extract::{Path, Query, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
fn now() -> i64 {
  crate::session::statistics::Timestamp::now().0 as i64
}
fn totals(rows: &[Value]) -> Value {
  let sum = |key: &str| rows.iter().map(|r| r[key].as_i64().unwrap_or(0)).sum::<i64>();
  json!({"usage_records":sum("with_usage"),"committed_responses":sum("completed"),"tokens":{"input_tokens":sum("input_tokens"),"output_tokens":sum("output_tokens"),"total_tokens":sum("total_tokens"),"reasoning_tokens":sum("reasoning_tokens")},"cache":{"read_input_tokens":sum("cached_input_tokens"),"write_input_tokens":sum("cache_write_input_tokens"),"request_hit_ratio":null}})
}
/// Optional window for the totals; all recorded usage when absent.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
  from_ms: Option<i64>,
  to_ms: Option<i64>,
}
async fn usage(app: Arc<App>, session: Option<String>, range: Range) -> Result<Json<Value>, ApiError> {
  let from = range.from_ms.unwrap_or(0);
  let to = range.to_ms.unwrap_or_else(|| now() + 1);
  if from < 0 || to <= from {
    return Err(ApiError::bad_request("invalid usage range"));
  }
  let index = app.index.clone();
  blocking(move||{
    let rows=index.usage_buckets(session.as_deref(),from,to,i64::MAX)?;
    let attempts=rows.iter().map(|r|r["attempts"].as_i64().unwrap_or(0)).sum::<i64>();
    let with_usage=rows.iter().map(|r|r["with_usage"].as_i64().unwrap_or(0)).sum::<i64>();
    Ok(Json(json!({"unit":"logical_model_call","statistics":{"model_attempts":attempts,"attempts_with_usage":with_usage,"attempts_without_usage":attempts-with_usage,"totals":totals(&rows),"by_provider_model":rows.iter().map(|r|json!({"provider":r["provider"],"model":r["model"],"totals":totals(std::slice::from_ref(r))})).collect::<Vec<_>>()}})))
  }).await
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
  app.index.read(&id)?;
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
    to - days * 86_400_000
  };
  let bucket = query.bucket.unwrap_or_else(|| "1d".into());
  let step = match bucket.as_str() {
    "1h" => 3_600_000,
    "6h" => 21_600_000,
    "1d" => 86_400_000,
    _ => return Err(ApiError::bad_request("unknown bucket")),
  };
  if from < 0 || to <= from || to - from > 366 * 86_400_000 {
    return Err(ApiError::bad_request("invalid time range"));
  }
  let index = app.index.clone();
  blocking(move || {
    let rows = index.usage_buckets(id.as_deref(), from, to, step)?;
    let samples = index.read_stream_samples(id.as_deref(), from, to)?;
    let displayed_samples = samples.len();
    let aggregates = index.aggregate_stream_samples(id.as_deref(), from, to, step)?;
    let sample_count: u64 = aggregates.iter().map(|row|row["count"].as_u64().unwrap()).sum();
    type Group = (Vec<Value>, Vec<Value>, Vec<Value>);
    let mut groups: BTreeMap<(String, String), Group> = BTreeMap::new();
    for row in rows {
      groups.entry((row["provider"].as_str().unwrap().into(), row["model"].as_str().unwrap().into())).or_default().0.push(row);
    }
    for sample in samples {
      groups.entry((sample["provider"].as_str().unwrap().into(), sample["model"].as_str().unwrap().into())).or_default().1.push(sample);
    }
    for row in aggregates {
      groups.entry((row["provider"].as_str().unwrap().into(), row["model"].as_str().unwrap().into())).or_default().2.push(row);
    }
    let (mut with_usage, mut completed, mut attempts) = (0, 0, 0);
    let mut output = Vec::new();
    for ((provider, model), (rows, mut samples, aggregates)) in groups {
      samples.sort_by_key(|sample|sample["at_ms"].as_i64().unwrap());
      with_usage += rows.iter().map(|r| r["with_usage"].as_i64().unwrap()).sum::<i64>();
      completed += rows.iter().map(|r| r["completed"].as_i64().unwrap()).sum::<i64>();
      attempts += rows.iter().map(|r| r["attempts"].as_i64().unwrap()).sum::<i64>();
      let indexed: BTreeMap<i64, &Value> = rows.iter().map(|r| (r["start_ms"].as_i64().unwrap(), r)).collect();
      let mut rates: BTreeMap<i64, (f64, u64, usize)> = BTreeMap::new();
      let (mut tokens, mut duration) = (0.0, 0u64);
      for sample in &aggregates {
        let entry = rates.entry(from + (sample["at_ms"].as_i64().unwrap() - from) / step * step).or_default();
        let count = sample["output_tokens"].as_f64().unwrap();
        let ms = sample["duration_ms"].as_u64().unwrap();
        entry.0 += count; entry.1 += ms; entry.2 += sample["count"].as_u64().unwrap() as usize;
        tokens += count; duration += ms;
      }
      let rate = |tokens: f64, duration: u64| if duration > 0 { Some(tokens * 1000.0 / duration as f64) } else { None };
      let mut buckets = Vec::new();
      let mut at = from;
      while at < to {
        let row = indexed.get(&at);
        let (sample_tokens, sample_duration, sample_count) = rates.get(&at).copied().unwrap_or_default();
        buckets.push(json!({"start_ms":at,"input_tokens":row.map(|r|r["input_tokens"].clone()).unwrap_or(json!(0)),"output_tokens":row.map(|r|r["output_tokens"].clone()).unwrap_or(json!(0)),"total_tokens":row.map(|r|r["total_tokens"].clone()).unwrap_or(json!(0)),"attempts":row.map(|r|r["attempts"].clone()).unwrap_or(json!(0)),"tps":rate(sample_tokens,sample_duration),"stream_samples":sample_count,"stream_output_tokens":sample_tokens,"stream_duration_ms":sample_duration}));
        at += step;
      }
      output.push(json!({"provider":provider,"model":model,"samples":samples,"window":{"total_tokens":totals(&rows)["tokens"]["total_tokens"],"tps":rate(tokens,duration)},"buckets":buckets}));
    }
    Ok(Json(json!({"unit":"logical_model_call","sampling":{"interval_ms":1000,"source":"estimated_visible_output","bytes_per_token":4,"displayed_samples":displayed_samples,"sample_limit":10000,"truncated":sample_count > displayed_samples as u64},"query":{"from_ms":from,"to_ms":to,"window":query.window,"bucket":bucket,"bucket_ms":step},"coverage":{"attempts_with_usage":with_usage,"attempts_success":completed,"attempts_failed":attempts-completed,"stream_samples":sample_count},"groups":output})))
  }).await
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
  app.index.read(&id)?;
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
async fn daily(app: Arc<App>, id: Option<String>, query: Daily) -> Result<Json<Value>, ApiError> {
  if !(1..=366).contains(&query.days) || !(-840..=840).contains(&query.tz_offset_minutes) {
    return Err(ApiError::bad_request("invalid days or timezone"));
  }
  let index = app.index.clone();
  blocking(move||{
    let db=rusqlite::Connection::open_in_memory()?;
    let end:Option<i64>=db.query_row("SELECT unixepoch(?1)*1000",[&query.end_date],|r|r.get(0))?;
    let last=end.ok_or_else(||ApiError::bad_request("invalid end_date"))?-query.tz_offset_minutes*60_000;
    let first=last-(query.days-1)*86_400_000;let to=last+86_400_000;
    let step=query.bucket_ms.unwrap_or(86_400_000);if step<=0||(to-first)/step>100_000{return Err(ApiError::bad_request("invalid bucket_ms"));}
    let rows=index.usage_buckets(id.as_deref(),first,to,step)?;let daily=index.usage_buckets(id.as_deref(),first,to,86_400_000)?;
    let mut buckets=Vec::new();let mut at=first;while at<to{buckets.push(json!({"start_ms":at,"total_tokens":rows.iter().filter(|r|r["start_ms"]==at).map(|r|r["total_tokens"].as_i64().unwrap()).sum::<i64>()}));at+=step;}
    let mut days=Vec::new();for n in 0..query.days{let at=first+n*86_400_000;let date:String=db.query_row("SELECT date(?1/1000,'unixepoch')",[at+query.tz_offset_minutes*60_000],|r|r.get(0))?;let rows:Vec<_>=daily.iter().filter(|r|r["start_ms"]==at).cloned().collect();let t=totals(&rows);days.push(json!({"date":date,"input_tokens":t["tokens"]["input_tokens"],"output_tokens":t["tokens"]["output_tokens"],"total_tokens":t["tokens"]["total_tokens"],"attempts":rows.iter().map(|r|r["attempts"].as_i64().unwrap()).sum::<i64>()}));}
    Ok(Json(json!({"query":{"days":query.days,"end_date":query.end_date,"tz_offset_minutes":query.tz_offset_minutes,"tz_label":format!("UTC{:+}",query.tz_offset_minutes as f64/60.0),"first_day_start_ms":first,"last_day_start_ms":last,"bucket_ms":step},"days":days,"buckets":buckets})))
  }).await
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
  app.index.read(&id)?;
  daily(app, Some(id), query).await
}
pub async fn status(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let sessions = app.sessions.lock().await;
  let mut active = 0;
  let mut ready = 0;
  let mut compacting = 0;
  let mut pending = 0;
  for session in sessions.values() {
    let snap = session.describe();
    active += i64::from(snap["status"]["running"] == true);
    ready += i64::from(snap["status"]["phase"] == "Ready");
    compacting += i64::from(snap["status"]["phase"] == "Compacting");
    pending += snap["status"]["queue_count"].as_u64().unwrap_or(0);
  }
  Ok(Json(
    json!({"counts":{"sessions":app.index.count()?,"runs":null},"queue":{"active_sessions":active,"ready_sessions":ready,"pending_items":pending,"compacting_sessions":compacting},"uptime_ms":app.started.elapsed().as_millis()}),
  ))
}
fn size(path: &std::path::Path) -> std::io::Result<u64> {
  if !path.exists() {
    return Ok(0);
  }
  if path.is_file() {
    return Ok(path.metadata()?.len());
  }
  let mut total = 0;
  for entry in std::fs::read_dir(path)? {
    let entry = entry?;
    if !entry.file_type()?.is_symlink() {
      total += size(&entry.path())?;
    }
  }
  Ok(total)
}
pub async fn storage(State(app): State<Arc<App>>) -> Result<Json<Value>, ApiError> {
  let dir = app.data_dir.clone();
  blocking(move||{
  let total=size(&dir).map_err(ApiError::internal)?;let blobs=size(&dir.join("blobs")).map_err(ApiError::internal)?;let executions=size(&dir.join("shell")).map_err(ApiError::internal)?;let data=size(&dir.join("wish.sqlite")).map_err(ApiError::internal)?;
  Ok(Json(json!({"bytes":{"total":total,"blobs":blobs,"executions":executions,"session_data":data,"service_data":total.saturating_sub(blobs+executions+data)},"counts":{"executions":null,"blobs":null,"image_jobs":null,"context_generations":null}})))
}).await
}
