//! Durable package catalog. A pending session release can be finished after restart.
use super::{ManagementStore, RepositoryError};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) const SCHEMA:&str = "
CREATE TABLE IF NOT EXISTS archives(id TEXT PRIMARY KEY,kind TEXT NOT NULL,session_id TEXT,batch_id TEXT,name TEXT NOT NULL,archived_at INTEGER NOT NULL,bytes INTEGER NOT NULL,manifest TEXT NOT NULL,expires INTEGER,pending TEXT);
CREATE TABLE IF NOT EXISTS session_releases(id TEXT PRIMARY KEY, pending TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS archive_time ON archives(archived_at DESC,id);
CREATE INDEX IF NOT EXISTS archive_session ON archives(session_id,archived_at DESC);
CREATE INDEX IF NOT EXISTS archive_batch ON archives(batch_id);
";
#[derive(Default, Deserialize)]
pub struct Filter {
  pub kind: Option<String>,
  pub session_id: Option<String>,
  pub batch_id: Option<String>,
  pub q: Option<String>,
  pub limit: Option<usize>,
  pub cursor: Option<String>,
  pub order: Option<String>,
}

#[derive(Deserialize)]
struct CatalogFields<'a> {
  archive_id: &'a str,
  kind: &'a str,
  session_id: Option<&'a str>,
  batch_id: Option<&'a str>,
  name: &'a str,
  archived_at: i64,
  archive_bytes: i64,
  expires_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct PendingRelease {
  pub temp_dir: Option<std::path::PathBuf>,
}

pub fn insert(
  db: &Connection,
  manifest: &Value,
  pending: Option<&PendingRelease>,
) -> Result<(), RepositoryError> {
  let fields = CatalogFields::deserialize(manifest).map_err(RepositoryError::internal)?;
  let journal =
    pending.map(serde_json::to_string).transpose().map_err(RepositoryError::internal)?;
  db.execute("INSERT INTO archives(id,kind,session_id,batch_id,name,archived_at,bytes,manifest,expires,pending) VALUES(?,?,?,?,?,?,?,?,?,?)",
    params![fields.archive_id,fields.kind,fields.session_id,fields.batch_id,fields.name,fields.archived_at,fields.archive_bytes,manifest.to_string(),fields.expires_at,journal])?;
  Ok(())
}
impl ManagementStore {
  pub fn archive_placement(&self, id: &str) -> Result<Value, RepositoryError> {
    let db = self.lock()?;
    let (folder, pinned): (Option<String>, bool) =
      db.query_row("SELECT folder,pinned FROM sessions WHERE id=?", [id], |row| {
        Ok((row.get(0)?, row.get(1)?))
      })?;
    let mut parent = folder.clone();
    let mut seen = std::collections::HashSet::new();
    let mut path = Vec::<String>::new();
    while let Some(id) = parent {
      if !seen.insert(id.clone()) {
        return Err(RepositoryError::internal("folder cycle"));
      }
      let (name, next) =
        db.query_row("SELECT name,parent FROM folders WHERE id=?", [id], |row| {
          Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
      path.push(name);
      parent = next;
    }
    path.reverse();
    Ok(json!({"folder":folder,"pinned":pinned,"folder_path":path}))
  }
  pub fn record_archive(
    &self,
    data: &Value,
    release: Option<&PendingRelease>,
  ) -> Result<Vec<String>, RepositoryError> {
    let mut db = self.lock()?;
    let tx = db.transaction()?;
    let mut evicted = Vec::new();
    if data["expires_at"].is_number() {
      let size = data["archive_bytes"]
        .as_i64()
        .ok_or_else(|| RepositoryError::invalid("invalid export size"))?;
      if size > 4 * 1024 * 1024 * 1024 {
        return Err(RepositoryError::invalid("export exceeds 4 GiB"));
      }
      let mut q = tx.prepare(
        "SELECT id,bytes FROM archives WHERE expires IS NOT NULL ORDER BY expires DESC,id",
      )?;
      let mut rows = q.query([])?;
      let mut bytes = size;
      let mut count = 1;
      while let Some(row) = rows.next()? {
        bytes += row.get::<_, i64>(1)?;
        count += 1;
        if count > 16 || bytes > 4 * 1024 * 1024 * 1024 {
          evicted.push(row.get::<_, String>(0)?);
        }
      }
      drop(rows);
      drop(q);
      for id in &evicted {
        tx.execute("DELETE FROM archives WHERE id=?", [id])?;
      }
    }
    insert(&tx, data, release)?;
    if release.is_some()
      && tx.execute("DELETE FROM sessions WHERE id=?", [data["session_id"].as_str()])? != 1
    {
      return Err(RepositoryError::Missing);
    }
    tx.commit()?;
    Ok(evicted)
  }
  pub fn list_archives(&self, f: Filter) -> Result<Value, RepositoryError> {
    let limit = f.limit.unwrap_or(50).clamp(1, 100);
    let cursor = f
      .cursor
      .as_deref()
      .map(|c| {
        c.split_once(':')
          .and_then(|(t, id)| Some((t.parse::<i64>().ok()?, id)))
          .ok_or_else(|| RepositoryError::invalid("invalid archive cursor"))
      })
      .transpose()?;
    let db = self.lock()?;
    let sort = match f.order.as_deref() {
      None | Some("time") => "archived_at",
      Some("size") => "bytes",
      _ => return Err(RepositoryError::invalid("invalid archive order")),
    };
    let filter = "expires IS NULL AND (?1 IS NULL OR kind=?1) AND (?2 IS NULL OR session_id=?2) AND (?3 IS NULL OR batch_id=?3) AND (?4 IS NULL OR instr(lower(name),lower(?4))>0)";
    let total: i64 = db.query_row(
      &format!("SELECT count(*) FROM archives WHERE {filter}"),
      params![f.kind, f.session_id, f.batch_id, f.q],
      |r| r.get(0),
    )?;
    let mut q = db.prepare(&format!("SELECT manifest FROM archives WHERE {filter} AND (?5 IS NULL OR {sort}<?5 OR ({sort}=?5 AND id>?6)) ORDER BY {sort} DESC,id LIMIT ?7"))?;
    let mut rows = q.query(params![
      f.kind,
      f.session_id,
      f.batch_id,
      f.q,
      cursor.map(|v| v.0),
      cursor.map(|v| v.1),
      (limit + 1) as i64
    ])?;
    let mut items = Vec::<Value>::new();
    while let Some(row) = rows.next()? {
      items
        .push(serde_json::from_str(&row.get::<_, String>(0)?).map_err(RepositoryError::internal)?);
    }
    let next = if items.len() > limit {
      items.pop();
      let last = &items[limit - 1];
      Some(format!(
        "{}:{}",
        last[if sort == "bytes" { "archive_bytes" } else { "archived_at" }],
        last["archive_id"].as_str().unwrap_or_default()
      ))
    } else {
      None
    };
    Ok(json!({"items":items,"next":next,"total":total}))
  }
  pub fn package(&self, id: &str, export: bool, now: u64) -> Result<Value, RepositoryError> {
    let raw:String=self.lock()?.query_row("SELECT manifest FROM archives WHERE id=?1 AND ((?2 AND expires>?3) OR (NOT ?2 AND expires IS NULL))",params![id,export,now as i64],|r|r.get(0)).optional()?.ok_or(RepositoryError::Missing)?;
    serde_json::from_str(&raw).map_err(RepositoryError::internal)
  }
  pub fn require_removable(&self, id: &str, export: bool) -> Result<(), RepositoryError> {
    match self
      .lock()?
      .query_row(
        "SELECT pending IS NULL FROM archives WHERE id=? AND (expires IS NOT NULL)=?",
        params![id, export],
        |r| r.get::<_, bool>(0),
      )
      .optional()?
    {
      Some(true) => Ok(()),
      Some(false) => Err(RepositoryError::Conflict("session release is pending".into())),
      None => Err(RepositoryError::Missing),
    }
  }
  pub fn remove_package(&self, id: &str, export: bool) -> Result<(), RepositoryError> {
    let changed = self.lock()?.execute(
      "DELETE FROM archives WHERE id=? AND pending IS NULL AND (expires IS NOT NULL)=?",
      params![id, export],
    )?;
    if changed == 0 {
      return Err(RepositoryError::Missing);
    }
    Ok(())
  }
  pub fn expired_exports(&self, now: u64) -> Result<Vec<String>, RepositoryError> {
    let db = self.lock()?;
    let mut q = db.prepare("SELECT id FROM archives WHERE expires<=?")?;
    Ok(q.query_map([now as i64], |r| r.get(0))?.collect::<Result<_, _>>()?)
  }
  pub fn pending_releases(&self) -> Result<Vec<(String, String, String)>, RepositoryError> {
    let db = self.lock()?;
    let mut q =
      db.prepare("SELECT id,session_id,pending FROM archives WHERE pending IS NOT NULL")?;
    let mut rows = q.query([])?;
    let mut result = Vec::new();
    while let Some(r) = rows.next()? {
      result.push((r.get(0)?, r.get(1)?, r.get(2)?));
    }
    Ok(result)
  }
  pub fn finish_release(&self, id: &str) -> Result<(), RepositoryError> {
    self.lock()?.execute("UPDATE archives SET pending=NULL WHERE id=?", [id])?;
    Ok(())
  }
  pub fn begin_delete(&self, id: &str, journal: &PendingRelease) -> Result<(), RepositoryError> {
    let mut db = self.lock()?;
    let tx = db.transaction()?;
    if tx.execute("DELETE FROM sessions WHERE id=?", [id])? != 1 {
      return Err(RepositoryError::Missing);
    }
    tx.execute(
      "INSERT INTO session_releases(id,pending) VALUES(?,?)",
      params![id, serde_json::to_string(journal).map_err(RepositoryError::internal)?],
    )?;
    tx.commit()?;
    Ok(())
  }
  pub fn pending_deletes(&self) -> Result<Vec<(String, String)>, RepositoryError> {
    let db = self.lock()?;
    let mut q = db.prepare("SELECT id,pending FROM session_releases")?;
    let mut rows = q.query([])?;
    let mut pending = Vec::new();
    while let Some(r) = rows.next()? {
      pending.push((r.get(0)?, r.get(1)?));
    }
    Ok(pending)
  }
  pub fn finish_delete(&self, id: &str) -> Result<(), RepositoryError> {
    self.lock()?.execute("DELETE FROM session_releases WHERE id=?", [id])?;
    Ok(())
  }
  pub fn releasing(&self, owner: &str) -> Result<bool, RepositoryError> {
    Ok(self.lock()?.query_row(
      "SELECT EXISTS(SELECT 1 FROM archives WHERE session_id=?1 AND pending IS NOT NULL UNION ALL SELECT 1 FROM session_releases WHERE id=?1)",
      [owner],
      |r| r.get(0),
    )?)
  }
}

// Freeze the exact orphan rows in SQLite's file-backed temp database. Compression
// then reads only private tables; new calls and samples can still be recorded.
pub struct UsageSnapshot {
  db: Connection,
  pub counts: [i64; 2],
}
const USAGE_TABLES: [&str; 2] = ["calls", "stream_samples"];
impl UsageSnapshot {
  pub fn open(path: &std::path::Path) -> Result<Self, RepositoryError> {
    let mut db = Connection::open(path)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch("PRAGMA temp_store=FILE")?;
    let tx = db.transaction()?;
    let mut counts = [0; 2];
    for (i, table) in USAGE_TABLES.iter().enumerate() {
      tx.execute_batch(&format!("CREATE TEMP TABLE old_{table} AS SELECT rowid AS source_row,* FROM main.{table} WHERE session NOT IN(SELECT id FROM sessions)"))?;
      counts[i] = tx.query_row(&format!("SELECT count(*) FROM old_{table}"), [], |r| r.get(0))?;
    }
    tx.commit()?;
    Ok(Self { db, counts })
  }
  pub fn visit<E: From<rusqlite::Error>>(
    &self,
    samples: bool,
    mut visit: impl FnMut(Value) -> Result<(), E>,
  ) -> Result<(), E> {
    let table = USAGE_TABLES[usize::from(samples)];
    let mut q = self.db.prepare(&format!("SELECT * FROM old_{table} ORDER BY source_row"))?;
    let names = q.column_names().iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut rows = q.query([])?;
    while let Some(row) = rows.next()? {
      let mut data = serde_json::Map::new();
      for (index, name) in names.iter().enumerate().skip(1) {
        use rusqlite::types::ValueRef;
        let value = match row.get_ref(index)? {
          ValueRef::Null => Value::Null,
          ValueRef::Integer(n) => json!(n),
          ValueRef::Real(n) => json!(n),
          _ => json!(row.get::<_, String>(index)?),
        };
        data.insert(name.clone(), value);
      }
      visit(Value::Object(data))?;
    }
    Ok(())
  }
  pub fn commit(
    mut self,
    store: &ManagementStore,
    manifest: &Value,
  ) -> Result<(), RepositoryError> {
    let guard = store.lock()?;
    let tx = self.db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for (i, table) in USAGE_TABLES.iter().enumerate() {
      // EXCEPT compares all original columns as well as rowid, including NULL values.
      let mismatch:bool=tx.query_row(&format!("SELECT EXISTS(SELECT * FROM old_{table} EXCEPT SELECT rowid,* FROM {table} WHERE rowid IN(SELECT source_row FROM old_{table}) AND session NOT IN(SELECT id FROM sessions))"),[],|r|r.get(0))?;
      if mismatch {
        return Err(RepositoryError::Conflict(
          "usage changed while archiving; retry cleanup".into(),
        ));
      }
      let deleted = tx.execute(
        &format!("DELETE FROM {table} WHERE rowid IN(SELECT source_row FROM old_{table})"),
        [],
      )?;
      if deleted as i64 != self.counts[i] {
        return Err(RepositoryError::internal("usage snapshot changed"));
      }
    }
    insert(&tx, manifest, None)?;
    tx.commit()?;
    if let Err(e) = super::shrink(&guard) {
      eprintln!("usage archive storage reclamation: {e}");
    }
    Ok(())
  }
}
