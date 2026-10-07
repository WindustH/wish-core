//! The usage statistics: every session's model calls, and one-second samples of each streamed
//! response's speed, merged past their limit; and the records deleted sessions left.
use super::{ManagementStore, shrink};
use crate::server::{error::ApiError, sampling};
use crate::{session::statistics::ModelCallRecord, storage::ReadList};
use rusqlite::{Row, params};
use serde::Serialize;
use std::{cmp::Reverse, collections::BinaryHeap, sync::atomic::Ordering};

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
  /// Saves the session's model calls from the last one saved on; those are rewritten, as a call
  /// may have ended since.
  pub fn save_calls(
    &self,
    session: &str,
    provider: &str,
    calls: &ReadList<ModelCallRecord>,
  ) -> Result<(), ApiError> {
    let last: Option<i64> = self.db.lock().unwrap().query_row(
      "SELECT max(position) FROM calls WHERE session=?1",
      [session],
      |row| row.get(0),
    )?;
    for page in calls.pages(last.unwrap_or(0) as u64) {
      let page = page?;
      let mut db = self.db.lock().unwrap();
      let tx = db.transaction()?;
      for record in &page.items {
        tx.execute(
          "INSERT INTO calls(session,position,provider,model,started_at,record) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(session,position) DO UPDATE SET record=excluded.record",
          params![
            session,
            record.id.0 as i64,
            provider,
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
    let db = self.db.lock().unwrap();
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
    self.db.lock().unwrap().execute(
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
    let db = self.db.lock().unwrap();
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
    let db = self.db.lock().unwrap();
    let mut statement = db.prepare("SELECT attempt_id,provider,model,at_ms,duration_ms,output_bytes FROM stream_samples WHERE (?1 IS NULL OR session=?1) AND at_ms>=?2 AND at_ms<?3 ORDER BY at_ms DESC,attempt_id DESC LIMIT ?4")?;
    let limit = sampling::LIST_LIMIT as i64;
    let rows = statement.query_map(params![session, from, to, limit], stream_sample)?;
    rows.map(|row| row.map_err(ApiError::from)).collect()
  }
  /// Sets the most stream samples kept; `None` keeps every one. [`Self::merge_stream_samples`]
  /// applies it.
  pub fn set_stream_sample_limit(&self, limit: Option<u64>) {
    self.sample_limit.store(limit.unwrap_or(0), Ordering::Relaxed);
  }
  /// How many stream samples there are.
  pub fn stream_sample_count(&self) -> Result<u64, ApiError> {
    let db = self.db.lock().unwrap();
    Ok(db.query_row("SELECT count(*) FROM stream_samples", [], |row| row.get::<_, i64>(0))? as u64)
  }
  /// Past the limit, merges neighbouring stream samples until nine-tenths of it remain, so a merge
  /// does not follow every call. A sample merges only with its neighbour in time among one
  /// session's samples of one model, so every session's and model's output and streaming time
  /// stay as they were: the later sample takes in the earlier one's bytes and time and keeps its
  /// place, where its interval ended. The pair whose merged sample would span the least time goes
  /// first, the older of equal ones, so dense stretches coarsen evenly and sparse and recent
  /// samples keep their detail longest. Returns how many samples merged away.
  pub fn merge_stream_samples(&self) -> Result<u64, ApiError> {
    let limit = self.sample_limit.load(Ordering::Relaxed);
    let mut db = self.db.lock().unwrap();
    let count =
      db.query_row("SELECT count(*) FROM stream_samples", [], |row| row.get::<_, i64>(0))?;
    if limit == 0 || count as u64 <= limit {
      return Ok(0);
    }
    let target = limit - limit / 10;
    struct Point {
      rowid: i64,
      /// Where the time it covers starts, which a merge moves back.
      start_ms: i64,
      at_ms: i64,
      duration_ms: i64,
      output_bytes: i64,
      merged: bool,
    }
    // In order within each series, which a neighbour must share.
    let mut points: Vec<Point> = Vec::with_capacity(count as usize);
    let mut starts = Vec::new();
    {
      let mut statement = db.prepare(
        "SELECT rowid,coalesce(session,''),provider,model,at_ms,duration_ms,output_bytes FROM \
         stream_samples ORDER BY coalesce(session,''),provider,model,at_ms",
      )?;
      let mut rows = statement.query([])?;
      let mut series: Option<(String, String, String)> = None;
      while let Some(row) = rows.next()? {
        let key = (row.get(1)?, row.get(2)?, row.get(3)?);
        if series.as_ref() != Some(&key) {
          starts.push(points.len());
          series = Some(key);
        }
        let (at_ms, duration_ms): (i64, i64) = (row.get(4)?, row.get(5)?);
        points.push(Point {
          rowid: row.get(0)?,
          start_ms: at_ms - duration_ms,
          at_ms,
          duration_ms,
          output_bytes: row.get(6)?,
          merged: false,
        });
      }
    }
    let first_of_series = |index: usize| starts.binary_search(&index).is_ok();
    let n = points.len();
    let mut previous: Vec<Option<usize>> =
      (0..n).map(|i| (!first_of_series(i)).then(|| i - 1)).collect();
    let mut next: Vec<Option<usize>> =
      (0..n).map(|i| (i + 1 < n && !first_of_series(i + 1)).then_some(i + 1)).collect();
    let mut gone = vec![false; n];
    // Pairs of neighbours by the time their merged sample would span, then how old: (span,
    // earlier's start, earlier, later). A merge only lengthens spans, so a pair that comes up with
    // an outdated one goes back with its current span, and a pair a merge took apart is skipped.
    let span = |points: &[Point], earlier: usize, later: usize| {
      points[later].at_ms - points[earlier].start_ms
    };
    let mut pairs = BinaryHeap::new();
    for (earlier, later) in next.iter().enumerate().filter_map(|(i, j)| j.map(|j| (i, j))) {
      pairs.push(Reverse((
        span(&points, earlier, later),
        points[earlier].start_ms,
        earlier,
        later,
      )));
    }
    let mut merged = 0u64;
    while count as u64 - merged > target {
      let Some(Reverse((spanned, _, earlier, later))) = pairs.pop() else { break };
      if gone[earlier] || gone[later] || next[earlier] != Some(later) {
        continue;
      }
      let current = span(&points, earlier, later);
      if current != spanned {
        pairs.push(Reverse((current, points[earlier].start_ms, earlier, later)));
        continue;
      }
      points[later].duration_ms += points[earlier].duration_ms;
      points[later].output_bytes += points[earlier].output_bytes;
      points[later].start_ms = points[earlier].start_ms;
      points[later].merged = true;
      gone[earlier] = true;
      merged += 1;
      previous[later] = previous[earlier];
      if let Some(before) = previous[earlier] {
        next[before] = Some(later);
        pairs.push(Reverse((span(&points, before, later), points[before].start_ms, before, later)));
      }
    }
    let tx = db.transaction()?;
    {
      let mut update =
        tx.prepare("UPDATE stream_samples SET duration_ms=?2,output_bytes=?3 WHERE rowid=?1")?;
      let mut delete = tx.prepare("DELETE FROM stream_samples WHERE rowid=?1")?;
      for (point, gone) in points.iter().zip(&gone) {
        if *gone {
          delete.execute([point.rowid])?;
        } else if point.merged {
          update.execute(params![point.rowid, point.duration_ms, point.output_bytes])?;
        }
      }
    }
    tx.commit()?;
    shrink(&db)?;
    Ok(merged)
  }

  /// What deleted sessions left in the usage records: their calls and stream samples. They stay
  /// in the usage statistics until [`Self::prune_usage`] clears them.
  pub fn leftover_usage(&self) -> Result<LeftoverUsage, ApiError> {
    let db = self.db.lock().unwrap();
    let count = |table: &str| {
      db.query_row(
        &format!("SELECT count(*) FROM {table} WHERE session NOT IN (SELECT id FROM sessions)"),
        [],
        |row| row.get::<_, i64>(0),
      )
    };
    Ok(LeftoverUsage {
      calls: count("calls")? as u64,
      stream_samples: count("stream_samples")? as u64,
    })
  }
  /// Deletes what deleted sessions left in the usage records, and hands its room back to the file
  /// system. Returns what went.
  pub fn prune_usage(&self) -> Result<LeftoverUsage, ApiError> {
    let mut db = self.db.lock().unwrap();
    let tx = db.transaction()?;
    let delete = |table: &str| {
      tx.execute(&format!("DELETE FROM {table} WHERE session NOT IN (SELECT id FROM sessions)"), [])
    };
    let pruned = LeftoverUsage {
      calls: delete("calls")? as u64,
      stream_samples: delete("stream_samples")? as u64,
    };
    tx.commit()?;
    shrink(&db)?;
    Ok(pruned)
  }
}

/// Usage records of sessions that no longer exist.
#[derive(Serialize)]
pub struct LeftoverUsage {
  pub calls: u64,
  pub stream_samples: u64,
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
