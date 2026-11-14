//! Retire groups and their independent transcripts. Session histories are untouched.
use super::Data;
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

pub const SUMMARY: &str = "remove groups and their separate message histories";

fn has_table(db: &Connection, name: &str) -> Result<bool, String> {
  db.query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1", [name], |_| Ok(()))
    .optional()
    .map(|row| row.is_some())
    .map_err(|error| error.to_string())
}

pub fn apply(data: &mut Data) -> Result<(), String> {
  if let Some(db) = data.management {
    // Persist cleanup work in the transaction. Files are removed only after it commits,
    // and startup resumes any cleanup interrupted between files.
    if has_table(db, "groups")? {
      db.execute_batch("CREATE TABLE IF NOT EXISTS retired_group_blobs(id TEXT PRIMARY KEY);
        INSERT OR IGNORE INTO retired_group_blobs SELECT id FROM groups WHERE id NOT IN (SELECT id FROM sessions);")
        .map_err(|error| error.to_string())?;
    }
    db.execute_batch("DROP TABLE IF EXISTS group_messages; DROP TABLE IF EXISTS group_members; DROP TABLE IF EXISTS groups;")
      .map_err(|error| error.to_string())?;
  }
  Ok(())
}

pub fn cleanup(directory: &Path) -> Result<(), String> {
  let database = directory.join("management.sqlite");
  if !database.is_file() {
    return Ok(());
  }
  let db = Connection::open(database).map_err(|error| error.to_string())?;
  if !has_table(&db, "retired_group_blobs")? {
    return Ok(());
  }
  let ids = db
    .prepare("SELECT id FROM retired_group_blobs")
    .and_then(|mut query| {
      query.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()
    })
    .map_err(|error| error.to_string())?;
  for id in ids {
    // Persisted IDs must never become arbitrary filesystem paths.
    if uuid::Uuid::parse_str(&id).is_err() {
      return Err(format!("invalid retired group ID: {id}"));
    }
    let path = directory.join("blobs").join(&id);
    let result = match std::fs::symlink_metadata(&path) {
      Ok(meta) if meta.is_symlink() => std::fs::remove_file(&path),
      Ok(_) => std::fs::remove_dir_all(&path),
      Err(error) => Err(error),
    };
    if let Err(error) = result
      && error.kind() != std::io::ErrorKind::NotFound
    {
      return Err(format!("remove retired group files {}: {error}", path.display()));
    }
    db.execute("DELETE FROM retired_group_blobs WHERE id=?1", [&id])
      .map_err(|error| error.to_string())?;
  }
  db.execute_batch("DROP TABLE retired_group_blobs").map_err(|error| error.to_string())
}
