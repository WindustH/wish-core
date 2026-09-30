//! The management index, `management.sqlite`: a record per session for the session list, every
//! session's model calls for the usage statistics, and one-second stream samples. Message and
//! history payloads stay in the engine's storage.
//!
//! A session's record is `{"session": descriptor, "status": status}`, as
//! `SessionSlot::persist_index` saves it; the list's filters read JSON paths out of it.
use crate::server::{error::ApiError, sampling, session::preview_pending_selection};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, sync::Mutex};

pub struct ManagementStore(Mutex<Connection>);
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SessionQuery {
  pub start: usize,
  pub limit: usize,
  pub query: String,
  pub phase: String,
  pub tag: String,
  pub order: String,
}
impl Default for SessionQuery {
  fn default() -> Self {
    Self {
      start: 0,
      limit: 50,
      query: String::new(),
      phase: String::new(),
      tag: String::new(),
      order: "desc".into(),
    }
  }
}

/// What the calls of one provider and model started within one time bucket add up to.
pub struct UsageBucket {
  pub provider: String,
  pub model: String,
  pub start_ms: i64,
  pub usage: Usage,
}
/// Model calls counted and their reported tokens summed.
#[derive(Clone, Copy, Default)]
pub struct Usage {
  pub attempts: i64,
  pub completed: i64,
  /// Calls whose provider reported usage.
  pub with_usage: i64,
  pub input_tokens: i64,
  pub output_tokens: i64,
  pub total_tokens: i64,
  pub cached_input_tokens: i64,
  pub cache_write_input_tokens: i64,
  pub reasoning_tokens: i64,
}
impl std::ops::Add for Usage {
  type Output = Self;
  fn add(self, other: Self) -> Self {
    Self {
      attempts: self.attempts + other.attempts,
      completed: self.completed + other.completed,
      with_usage: self.with_usage + other.with_usage,
      input_tokens: self.input_tokens + other.input_tokens,
      output_tokens: self.output_tokens + other.output_tokens,
      total_tokens: self.total_tokens + other.total_tokens,
      cached_input_tokens: self.cached_input_tokens + other.cached_input_tokens,
      cache_write_input_tokens: self.cache_write_input_tokens + other.cache_write_input_tokens,
      reasoning_tokens: self.reasoning_tokens + other.reasoning_tokens,
    }
  }
}
impl<'a> std::iter::Sum<&'a UsageBucket> for Usage {
  fn sum<I: Iterator<Item = &'a UsageBucket>>(buckets: I) -> Self {
    buckets.fold(Self::default(), |sum, bucket| sum + bucket.usage)
  }
}
/// The stream samples of one provider and model within one time bucket.
pub struct StreamAggregate {
  pub provider: String,
  pub model: String,
  pub at_ms: i64,
  pub output_tokens: f64,
  pub duration_ms: i64,
  pub count: i64,
}
/// One stream sample, as the usage series lists it.
#[derive(Serialize)]
pub struct StreamSample {
  pub attempt_id: String,
  pub provider: String,
  pub model: String,
  pub at_ms: i64,
  pub duration_ms: i64,
  pub output_bytes: i64,
  pub output_tokens: f64,
  pub tps: f64,
  pub source: &'static str,
}

impl ManagementStore {
  pub fn open(path: &Path) -> Result<Self, ApiError> {
    let db = Connection::open(path)?;
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, updated_at INTEGER NOT NULL, record TEXT NOT NULL); CREATE INDEX IF NOT EXISTS sessions_updated ON sessions(updated_at DESC,id); CREATE TABLE IF NOT EXISTS calls(session TEXT NOT NULL,position INTEGER NOT NULL,provider TEXT NOT NULL,model TEXT NOT NULL,started_at INTEGER NOT NULL,record TEXT NOT NULL,PRIMARY KEY(session,position)); CREATE INDEX IF NOT EXISTS calls_time ON calls(started_at); CREATE INDEX IF NOT EXISTS calls_session_time ON calls(session,started_at);")?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS stream_samples(attempt_id TEXT NOT NULL,session TEXT,provider TEXT NOT NULL,model TEXT NOT NULL,at_ms INTEGER NOT NULL,duration_ms INTEGER NOT NULL,output_bytes INTEGER NOT NULL,PRIMARY KEY(attempt_id,at_ms)); CREATE INDEX IF NOT EXISTS stream_samples_time ON stream_samples(at_ms); CREATE INDEX IF NOT EXISTS stream_samples_session_time ON stream_samples(session,at_ms);")?;
    Ok(Self(Mutex::new(db)))
  }
  pub fn save(&self, record: &Value) -> Result<(), ApiError> {
    self.0.lock().unwrap().execute(
      "INSERT INTO sessions(id,updated_at,record) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET updated_at=excluded.updated_at,record=excluded.record",
      params![
        record["session"]["id"].as_str(),
        record["session"]["updated_at"].as_i64(),
        record.to_string()
      ],
    )?;
    Ok(())
  }
  pub fn read(&self, id: &str) -> Result<Value, ApiError> {
    let value: String = self
      .0
      .lock()
      .unwrap()
      .query_row("SELECT record FROM sessions WHERE id=?1", [id], |r| r.get(0))
      .optional()?
      .ok_or_else(ApiError::not_found)?;
    serde_json::from_str(&value).map_err(ApiError::internal)
  }
  pub fn exists(&self, id: &str) -> Result<bool, ApiError> {
    let found = self
      .0
      .lock()
      .unwrap()
      .query_row("SELECT 1 FROM sessions WHERE id=?1", [id], |_| Ok(()))
      .optional()?;
    Ok(found.is_some())
  }
  /// A page of the session list, each session as the API shows it.
  pub fn list(&self, q: SessionQuery) -> Result<Value, ApiError> {
    if q.limit == 0 {
      return Err(ApiError::bad_request("limit must be positive"));
    }
    let direction = match q.order.as_str() {
      "asc" => "ASC",
      "desc" => "DESC",
      _ => return Err(ApiError::bad_request("order must be asc or desc")),
    };
    let sql = format!(
      "SELECT record FROM sessions WHERE (?1='' OR instr(lower(json_extract(record,'$.session.name')),lower(?1))>0 OR instr(lower(id),lower(?1))>0) AND (?2='' OR json_extract(record,'$.status.phase')=?2 OR (?2='running' AND json_extract(record,'$.status.running')=1)) AND (?3='' OR EXISTS(SELECT 1 FROM json_each(json_extract(record,'$.status.metadata.tags')) WHERE value=?3)) ORDER BY updated_at {direction},id {direction} LIMIT ?4 OFFSET ?5"
    );
    let db = self.0.lock().unwrap();
    let mut statement = db.prepare(&sql)?;
    let rows = statement.query_map(
      params![
        q.query,
        q.phase,
        q.tag,
        i64::try_from(q.limit.saturating_add(1)).map_err(ApiError::internal)?,
        q.start as i64
      ],
      |r| r.get::<_, String>(0),
    )?;
    let mut items = Vec::new();
    for row in rows {
      let mut record: Value = serde_json::from_str(&row?).map_err(ApiError::internal)?;
      preview_pending_selection(&mut record);
      items.push(record);
    }
    let more = items.len() > q.limit;
    items.truncate(q.limit);
    Ok(json!({"items":items,"next":more.then_some(q.start+q.limit)}))
  }
  pub fn delete(&self, id: &str) -> Result<(), ApiError> {
    self.0.lock().unwrap().execute("DELETE FROM sessions WHERE id=?1", [id])?;
    Ok(())
  }
  /// Every session's record, newest first.
  pub fn list_all(&self) -> Result<Vec<Value>, ApiError> {
    let db = self.0.lock().unwrap();
    let mut statement =
      db.prepare("SELECT record FROM sessions ORDER BY updated_at DESC, id DESC")?;
    let rows = statement.query_map([], |r| r.get::<_, String>(0))?;
    let mut records = Vec::new();
    for row in rows {
      records.push(serde_json::from_str(&row?).map_err(ApiError::internal)?);
    }
    Ok(records)
  }
  pub fn count(&self) -> Result<i64, ApiError> {
    Ok(self.0.lock().unwrap().query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))?)
  }

  /// Saves the session's model calls from the last one saved on; those are rewritten, as a call
  /// may have ended since.
  pub fn save_calls(
    &self,
    descriptor: &crate::server::session::Descriptor,
    calls: &crate::storage::ReadList<crate::session::statistics::ModelCallRecord>,
  ) -> Result<(), ApiError> {
    let last: Option<i64> = self.0.lock().unwrap().query_row(
      "SELECT max(position) FROM calls WHERE session=?1",
      [&descriptor.id],
      |row| row.get(0),
    )?;
    for page in calls.pages(last.unwrap_or(0) as u64) {
      let page = page?;
      let mut db = self.0.lock().unwrap();
      let tx = db.transaction()?;
      for record in &page.items {
        tx.execute(
          "INSERT INTO calls(session,position,provider,model,started_at,record) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(session,position) DO UPDATE SET record=excluded.record",
          params![
            descriptor.id,
            record.id.0 as i64,
            descriptor.provider,
            record.model,
            record.started_at.0 as i64,
            serde_json::to_string(record).map_err(ApiError::internal)?
          ],
        )?;
      }
      tx.commit()?;
    }
    Ok(())
  }
  /// The calls started in `[from, to)`, one session's or all, by provider, model and bucket.
  pub fn usage_buckets(
    &self,
    session: Option<&str>,
    from: i64,
    to: i64,
    bucket: i64,
  ) -> Result<Vec<UsageBucket>, ApiError> {
    let db = self.0.lock().unwrap();
    let mut statement = db.prepare("SELECT provider,model,?2+((started_at-?2)/?4)*?4, count(*),sum(json_extract(record,'$.status')='Completed'), sum(json_extract(record,'$.usage.input_tokens') IS NOT NULL OR json_extract(record,'$.usage.output_tokens') IS NOT NULL),sum(coalesce(json_extract(record,'$.usage.input_tokens'),0)),sum(coalesce(json_extract(record,'$.usage.output_tokens'),0)),sum(coalesce(json_extract(record,'$.usage.total_tokens'),json_extract(record,'$.usage.input_tokens')+json_extract(record,'$.usage.output_tokens'),0)),sum(coalesce(json_extract(record,'$.usage.cached_input_tokens'),0)),sum(coalesce(json_extract(record,'$.usage.cache_write_input_tokens'),0)),sum(coalesce(json_extract(record,'$.usage.reasoning_tokens'),0)) FROM calls WHERE (?1 IS NULL OR session=?1) AND started_at>=?2 AND started_at<?3 GROUP BY provider,model,3 ORDER BY 3")?;
    let rows = statement.query_map(params![session, from, to, bucket], |row| {
      Ok(UsageBucket {
        provider: row.get(0)?,
        model: row.get(1)?,
        start_ms: row.get(2)?,
        usage: Usage {
          attempts: row.get(3)?,
          completed: row.get(4)?,
          with_usage: row.get(5)?,
          input_tokens: row.get(6)?,
          output_tokens: row.get(7)?,
          total_tokens: row.get(8)?,
          cached_input_tokens: row.get(9)?,
          cache_write_input_tokens: row.get(10)?,
          reasoning_tokens: row.get(11)?,
        },
      })
    })?;
    rows.map(|row| row.map_err(ApiError::from)).collect()
  }

  pub fn save_stream_sample(&self, sample: &sampling::Sample) -> Result<(), ApiError> {
    self.0.lock().unwrap().execute(
      "INSERT INTO stream_samples(attempt_id,session,provider,model,at_ms,duration_ms,output_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7)",
      params![
        sample.attempt_id,
        sample.session,
        sample.provider,
        sample.model,
        sample.at_ms as i64,
        sample.duration_ms as i64,
        sample.output_bytes as i64
      ],
    )?;
    Ok(())
  }
  /// The samples in `[from, to)`, one session's or all, by provider, model and step.
  pub fn aggregate_stream_samples(
    &self,
    session: Option<&str>,
    from: i64,
    to: i64,
    step: i64,
  ) -> Result<Vec<StreamAggregate>, ApiError> {
    let db = self.0.lock().unwrap();
    let mut statement = db.prepare("SELECT provider,model,?2+(at_ms-?2)/?4*?4,SUM(output_bytes),SUM(duration_ms),COUNT(*) FROM stream_samples WHERE (?1 IS NULL OR session=?1) AND at_ms>=?2 AND at_ms<?3 GROUP BY provider,model,3")?;
    let rows = statement.query_map(params![session, from, to, step], |row| {
      Ok(StreamAggregate {
        provider: row.get(0)?,
        model: row.get(1)?,
        at_ms: row.get(2)?,
        output_tokens: tokens(row.get(3)?),
        duration_ms: row.get(4)?,
        count: row.get(5)?,
      })
    })?;
    rows.map(|row| row.map_err(ApiError::from)).collect()
  }
  /// The newest samples in `[from, to)`, one session's or all, up to the series' limit.
  pub fn read_stream_samples(
    &self,
    session: Option<&str>,
    from: i64,
    to: i64,
  ) -> Result<Vec<StreamSample>, ApiError> {
    let db = self.0.lock().unwrap();
    let mut statement = db.prepare("SELECT attempt_id,provider,model,at_ms,duration_ms,output_bytes FROM stream_samples WHERE (?1 IS NULL OR session=?1) AND at_ms>=?2 AND at_ms<?3 ORDER BY at_ms DESC,attempt_id DESC LIMIT ?4")?;
    let limit = sampling::LIST_LIMIT as i64;
    let rows = statement.query_map(params![session, from, to, limit], stream_sample)?;
    rows.map(|row| row.map_err(ApiError::from)).collect()
  }
}

fn stream_sample(row: &Row) -> rusqlite::Result<StreamSample> {
  let (duration_ms, output_bytes): (i64, i64) = (row.get(4)?, row.get(5)?);
  let output_tokens = tokens(output_bytes);
  Ok(StreamSample {
    attempt_id: row.get(0)?,
    provider: row.get(1)?,
    model: row.get(2)?,
    at_ms: row.get(3)?,
    duration_ms,
    output_bytes,
    output_tokens,
    tps: output_tokens * 1000.0 / duration_ms as f64,
    source: sampling::SOURCE,
  })
}

/// Estimated tokens in received bytes.
fn tokens(bytes: i64) -> f64 {
  bytes as f64 / sampling::BYTES_PER_TOKEN as f64
}
