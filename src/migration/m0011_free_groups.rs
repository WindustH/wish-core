//! 10 to 11: sessions in a group wake one another as the user wakes them, and end a conversation by
//! keeping silent, so the relay limit goes: `groups` leaves the configuration, and the group
//! tables lose each group's count of relays and the notes Wish wrote when the limit paused one.

use super::Data;
use rusqlite::{Connection, OptionalExtension};

pub const SUMMARY: &str = "let sessions in a group wake one another without a limit";

pub fn apply(data: &mut Data) -> Result<(), String> {
  if let Some(config) = data.config.as_object_mut() {
    config.remove("groups");
  }
  let Some(management) = data.management else { return Ok(()) };
  let failed = |error: rusqlite::Error| error.to_string();
  if has_column(management, "group_messages", "note").map_err(failed)? {
    management
      .execute_batch(
        "DELETE FROM group_messages WHERE author_kind='wish';
         ALTER TABLE group_messages DROP COLUMN note;",
      )
      .map_err(failed)?;
  }
  if has_column(management, "groups", "relays").map_err(failed)? {
    management.execute_batch("ALTER TABLE groups DROP COLUMN relays").map_err(failed)?;
  }
  Ok(())
}

/// Whether `table` still has `column`: a step run again after a crash finds it gone.
fn has_column(db: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
  db.query_row("SELECT 1 FROM pragma_table_info(?1) WHERE name=?2", [table, column], |_| Ok(()))
    .optional()
    .map(|found| found.is_some())
}
