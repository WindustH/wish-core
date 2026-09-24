//! Application session index. Message/history payloads remain exclusively in wish-core storage.
use crate::server::error::ApiError;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
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
impl ManagementStore {
  pub fn open(path: &Path) -> Result<Self, ApiError> {
    let db = Connection::open(path)?;
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, updated_at INTEGER NOT NULL, record TEXT NOT NULL); CREATE INDEX IF NOT EXISTS sessions_updated ON sessions(updated_at DESC,id); CREATE TABLE IF NOT EXISTS calls(session TEXT NOT NULL,position INTEGER NOT NULL,provider TEXT NOT NULL,model TEXT NOT NULL,started_at INTEGER NOT NULL,record TEXT NOT NULL,PRIMARY KEY(session,position)); CREATE INDEX IF NOT EXISTS calls_time ON calls(started_at); CREATE INDEX IF NOT EXISTS calls_session_time ON calls(session,started_at);")?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS stream_samples(attempt_id TEXT NOT NULL,session TEXT,provider TEXT NOT NULL,model TEXT NOT NULL,at_ms INTEGER NOT NULL,duration_ms INTEGER NOT NULL,output_bytes INTEGER NOT NULL,PRIMARY KEY(attempt_id,at_ms)); CREATE INDEX IF NOT EXISTS stream_samples_time ON stream_samples(at_ms); CREATE INDEX IF NOT EXISTS stream_samples_session_time ON stream_samples(session,at_ms);")?;
    Ok(Self(Mutex::new(db)))
  }
  pub fn save(&self, record: &Value) -> Result<(), ApiError> {
    self.0.lock().unwrap().execute("INSERT INTO sessions(id,updated_at,record) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET updated_at=excluded.updated_at,record=excluded.record",
      params![record["session"]["id"].as_str(),record["session"]["updated_at"].as_i64(),record.to_string()])?;
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
    serde_json::from_str(&value).map(project_session_record).map_err(ApiError::internal)
  }
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
      items.push(project_session_record(serde_json::from_str::<Value>(&row?).map_err(ApiError::internal)?));
    }
    let more = items.len() > q.limit;
    items.truncate(q.limit);
    Ok(json!({"items":items,"next":more.then_some(q.start+q.limit)}))
  }
  pub fn delete(&self, id: &str) -> Result<(), ApiError> {
    self.0.lock().unwrap().execute("DELETE FROM sessions WHERE id=?1", [id])?;
    Ok(())
  }
  pub fn count(&self) -> Result<i64, ApiError> {
    Ok(self.0.lock().unwrap().query_row("SELECT count(*) FROM sessions", [], |r| r.get(0))?)
  }
}
fn project_session_record(mut record: Value) -> Value {
  if let Some(outcome) = record.pointer_mut("/status/last_operation/outcome") {
    if let Some(partial) = outcome.get_mut("StreamFailed") {
      let reason = partial.get("reason").cloned().unwrap_or(Value::Null);
      *partial = json!({"reason":reason});
    }
    if let Some(response) = outcome.get_mut("ModelStopped") {
      let stop_reason = response.get("stop_reason").cloned().unwrap_or(Value::Null);
      *response = json!({"stop_reason":stop_reason});
    }
  }
  record
}
impl From<rusqlite::Error> for ApiError {
  fn from(e: rusqlite::Error) -> Self {
    Self::internal(e)
  }
}

impl ManagementStore {
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
    let mut start = last.unwrap_or(0) as u64;
    loop {
      let page = calls.read_page(start, 128)?;
      let mut db = self.0.lock().unwrap();
      let tx = db.transaction()?;
      for record in &page.items {
        tx.execute("INSERT INTO calls(session,position,provider,model,started_at,record) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(session,position) DO UPDATE SET record=excluded.record",
          params![descriptor.id,record.id.0 as i64,descriptor.provider,record.model,record.started_at.0 as i64,serde_json::to_string(record).map_err(ApiError::internal)?])?;
      }
      tx.commit()?;
      match page.next {
        Some(next) => start = next,
        None => break,
      }
    }
    Ok(())
  }
  pub fn usage_buckets(
    &self,
    session: Option<&str>,
    from: i64,
    to: i64,
    bucket: i64,
  ) -> Result<Vec<Value>, ApiError> {
    let db = self.0.lock().unwrap();
    let mut statement=db.prepare("SELECT provider,model,?2+((started_at-?2)/?4)*?4, count(*),sum(json_extract(record,'$.status')='Completed'), sum(json_extract(record,'$.usage.input_tokens') IS NOT NULL OR json_extract(record,'$.usage.output_tokens') IS NOT NULL),sum(coalesce(json_extract(record,'$.usage.input_tokens'),0)),sum(coalesce(json_extract(record,'$.usage.output_tokens'),0)),sum(coalesce(json_extract(record,'$.usage.total_tokens'),json_extract(record,'$.usage.input_tokens')+json_extract(record,'$.usage.output_tokens'),0)),sum(coalesce(json_extract(record,'$.usage.cached_input_tokens'),0)),sum(coalesce(json_extract(record,'$.usage.cache_write_input_tokens'),0)),sum(coalesce(json_extract(record,'$.usage.reasoning_tokens'),0)) FROM calls WHERE (?1 IS NULL OR session=?1) AND started_at>=?2 AND started_at<?3 GROUP BY provider,model,3 ORDER BY 3")?;
    let rows=statement.query_map(params![session,from,to,bucket],|r|Ok(json!({"provider":r.get::<_,String>(0)?,"model":r.get::<_,String>(1)?,"start_ms":r.get::<_,i64>(2)?,"attempts":r.get::<_,i64>(3)?,"completed":r.get::<_,i64>(4)?,"with_usage":r.get::<_,i64>(5)?,"input_tokens":r.get::<_,i64>(6)?,"output_tokens":r.get::<_,i64>(7)?,"total_tokens":r.get::<_,i64>(8)?,"cached_input_tokens":r.get::<_,i64>(9)?,"cache_write_input_tokens":r.get::<_,i64>(10)?,"reasoning_tokens":r.get::<_,i64>(11)?})))?;
    rows.map(|row| row.map_err(ApiError::from)).collect()
  }
}

impl ManagementStore {
  pub fn save_stream_sample(
    &self,
    sample: &crate::server::sampling::Sample,
  ) -> Result<(), ApiError> {
    self.0.lock().unwrap().execute("INSERT INTO stream_samples(attempt_id,session,provider,model,at_ms,duration_ms,output_bytes) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![sample.attempt_id,sample.session,sample.provider,sample.model,sample.at_ms as i64,sample.duration_ms as i64,sample.output_bytes as i64])?;
    Ok(())
  }
  pub fn aggregate_stream_samples(
    &self,
    session: Option<&str>,
    from: i64,
    to: i64,
    step: i64,
  ) -> Result<Vec<Value>, ApiError> {
    let db = self.0.lock().unwrap();
    let mut statement = db.prepare("SELECT provider,model,?2+(at_ms-?2)/?4*?4,SUM(output_bytes),SUM(duration_ms),COUNT(*) FROM stream_samples WHERE (?1 IS NULL OR session=?1) AND at_ms>=?2 AND at_ms<?3 GROUP BY provider,model,3")?;
    let rows = statement.query_map(params![session,from,to,step], |row| {
      Ok(json!({"provider":row.get::<_,String>(0)?,"model":row.get::<_,String>(1)?,"at_ms":row.get::<_,i64>(2)?,"output_tokens":row.get::<_,i64>(3)? as f64/4.0,"duration_ms":row.get::<_,i64>(4)?,"count":row.get::<_,i64>(5)?}))
    })?;
    rows.map(|row| row.map_err(ApiError::from)).collect()
  }
  pub fn read_stream_samples(
    &self,
    session: Option<&str>,
    from: i64,
    to: i64,
  ) -> Result<Vec<Value>, ApiError> {
    let db = self.0.lock().unwrap();
    let mut statement = db.prepare("SELECT attempt_id,provider,model,at_ms,duration_ms,output_bytes FROM stream_samples WHERE (?1 IS NULL OR session=?1) AND at_ms>=?2 AND at_ms<?3 ORDER BY at_ms DESC,attempt_id DESC LIMIT 10000")?;
    let rows = statement.query_map(params![session,from,to], |row| {
      let bytes: i64 = row.get(5)?;
      let duration: i64 = row.get(4)?;
      Ok(json!({"attempt_id":row.get::<_,String>(0)?,"provider":row.get::<_,String>(1)?,"model":row.get::<_,String>(2)?,"at_ms":row.get::<_,i64>(3)?,"duration_ms":duration,"output_bytes":bytes,"output_tokens":bytes as f64/4.0,"tps":bytes as f64*250.0/duration as f64,"source":"estimated_visible_output"}))
    })?;
    rows.map(|row| row.map_err(ApiError::from)).collect()
  }
}
