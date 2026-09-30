//! 3 to 4: deleting a session now hands its room back to the file system. That needs the engine
//! database in incremental auto-vacuum mode, and its search index merged: until now a delete only
//! marked the index's rows gone, and their text stayed. The index is merged here and the mode set;
//! the VACUUM that closes every migration rebuilds the file in it.

use super::Data;

pub const SUMMARY: &str = "compact the history search index so deleting a session shrinks storage";

pub fn apply(data: &mut Data) -> Result<(), String> {
  let Some(wish) = data.wish else { return Ok(()) };
  let failed = |error: rusqlite::Error| error.to_string();
  wish.execute_batch("PRAGMA auto_vacuum=INCREMENTAL").map_err(failed)?;
  let indexed: bool = wish
    .query_row(
      "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='wish_history_words')",
      [],
      |row| row.get(0),
    )
    .map_err(failed)?;
  if indexed {
    wish
      .execute_batch(
        "INSERT INTO wish_history_words(wish_history_words) VALUES('optimize');
        INSERT INTO wish_history_substrings(wish_history_substrings) VALUES('optimize');",
      )
      .map_err(failed)?;
  }
  Ok(())
}
