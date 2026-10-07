//! 7 to 8: clearing the usage records deleted sessions left, and merging stream samples past their
//! limit, hand their room back to the file system. That needs the management database in
//! incremental auto-vacuum mode: the mode is set here, and the VACUUM that closes every migration
//! rebuilds the file in it.

use super::Data;

pub const SUMMARY: &str = "let the management index shrink when usage records are cleared";

pub fn apply(data: &mut Data) -> Result<(), String> {
  let Some(management) = data.management else { return Ok(()) };
  management.execute_batch("PRAGMA auto_vacuum=INCREMENTAL").map_err(|error| error.to_string())
}
