//! 11 to 12: the list gets folders. Sessions and groups gain `folder`, the folder each is in, and
//! `pinned`; everything kept so far is in the root, unpinned. The `folders` table and the indexes a
//! folder's listing is read along are made as a new file gets them, when the index opens.

use super::Data;
use rusqlite::{Connection, OptionalExtension};

pub const SUMMARY: &str = "arrange sessions and groups in folders, everything so far in the root";

pub fn apply(data: &mut Data) -> Result<(), String> {
  let Some(management) = data.management else { return Ok(()) };
  let failed = |error: rusqlite::Error| error.to_string();
  for table in ["sessions", "groups"] {
    for (column, definition) in [("folder", "TEXT"), ("pinned", "INTEGER NOT NULL DEFAULT 0")] {
      // A step run again after a crash finds the column there.
      if !has_column(management, table, column).map_err(failed)? {
        management
          .execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"))
          .map_err(failed)?;
      }
    }
  }
  Ok(())
}

fn has_column(db: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
  db.query_row("SELECT 1 FROM pragma_table_info(?1) WHERE name=?2", [table, column], |_| Ok(()))
    .optional()
    .map(|found| found.is_some())
}
