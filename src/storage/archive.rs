//! Bounded keyset reads of authoritative rows. No compression runs on the database worker.
use super::StorageError;
use rusqlite::{Connection, OpenFlags, params};
use std::path::Path;
pub struct ArchiveReader {
  db: Connection,
  key: String,
}
impl ArchiveReader {
  pub fn open(path: &Path, key: String) -> Result<Self, StorageError> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    let present: bool =
      db.query_row("SELECT EXISTS(SELECT 1 FROM wish_objects WHERE name=?)", [&key], |r| r.get(0))?;
    if !present {
      return Err(StorageError::NotFound(key));
    }
    Ok(Self { db, key })
  }
  pub fn version(&self) -> Result<u32, StorageError> {
    Ok(self.db.query_row("PRAGMA user_version", [], |r| r.get(0))?)
  }
  pub fn objects(&self, after: &str) -> Result<Vec<(String, String, Vec<u8>)>, StorageError> {
    let mut q=self.db.prepare("SELECT name,kind,value FROM wish_objects WHERE (name=?1 OR (name>=?2 AND name<?3)) AND name>?4 ORDER BY name LIMIT 64")?;
    let mut rows =
      q.query(params![self.key, format!("{}/", self.key), format!("{}0", self.key), after])?;
    let mut out = vec![];
    let mut size = 0;
    while let Some(row) = rows.next()? {
      let raw: Vec<u8> = row.get(2)?;
      size += raw.len();
      out.push((row.get(0)?, row.get(1)?, raw));
      if size >= 262144 {
        break;
      }
    }
    Ok(out)
  }
  pub fn lists(&self) -> Result<Vec<(i64, String, String, i64)>, StorageError> {
    let mut q=self.db.prepare("SELECT k.id,k.name,l.kind,l.length FROM wish_list_keys k JOIN wish_lists l USING(name) WHERE k.name>=? AND k.name<? ORDER BY k.name")?;
    Ok(
      q.query_map(params![format!("{}/", self.key), format!("{}0", self.key)], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
      })?
      .collect::<Result<_, _>>()?,
    )
  }
  pub fn items(
    &self,
    list: i64,
    start: u64,
    end: u64,
  ) -> Result<Vec<(u64, Vec<u8>)>, StorageError> {
    let mut q=self.db.prepare("SELECT position,value FROM wish_items WHERE list=? AND position>=? AND position<=? ORDER BY position LIMIT 256")?;
    let mut rows = q.query(params![list, start as i64, end as i64])?;
    let mut out = vec![];
    let mut size = 0;
    while let Some(row) = rows.next()? {
      let raw: Vec<u8> = row.get(1)?;
      size += raw.len();
      out.push((row.get::<_, i64>(0)? as u64, raw));
      if size >= 262144 {
        break;
      }
    }
    Ok(out)
  }
}
