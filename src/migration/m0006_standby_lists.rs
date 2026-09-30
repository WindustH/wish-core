//! 5 to 6: a standby generation's entry list is now deleted when the standby is given a new one
//! without being activated - when it is prepared again, emptied, or replaced by the trimmed context
//! of a cutover. Nothing reads such a list: a generation is read through the list its record names
//! now, and the session's queue is the only other entry list. The lists earlier builds left behind
//! are deleted here: in each session, every entry list that neither its queue nor one of its
//! generations names. A session whose record names no generations list that exists is left as it
//! is, and a generation that names no entry list stops the migration.

use super::Data;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashSet;

pub const SUMMARY: &str = "delete the entry lists replaced standby generations left behind";

const SESSION_RECORD: &str = "wish_core::session::persistence::SessionRecord";
const ENTRY_LIST: &str = "wish_core::session::history::entry::EntryId";

pub fn apply(data: &mut Data) -> Result<(), String> {
  let Some(wish) = data.wish else { return Ok(()) };
  let failed = |error: rusqlite::Error| error.to_string();
  let sessions = wish
    .prepare("SELECT name, value FROM wish_objects WHERE kind=?1")
    .and_then(|mut statement| {
      statement
        .query_map([SESSION_RECORD], |row| {
          Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()
    })
    .map_err(failed)?;
  for (key, record) in sessions {
    let record: Value =
      serde_json::from_slice(&record).map_err(|error| format!("{key}: {error}"))?;
    let Some(named) = named_lists(wish, &record).map_err(|error| format!("{key}: {error}"))? else {
      continue;
    };
    let lists = wish
      .prepare("SELECT name FROM wish_lists WHERE kind=?1 AND substr(name,1,length(?2))=?2")
      .and_then(|mut statement| {
        statement
          .query_map([ENTRY_LIST, &format!("{key}/")], |row| row.get::<_, String>(0))?
          .collect::<Result<Vec<_>, _>>()
      })
      .map_err(failed)?;
    for name in lists.iter().filter(|name| !named.contains(*name)) {
      wish
        .execute(
          "DELETE FROM wish_items WHERE list=(SELECT id FROM wish_list_keys WHERE name=?1)",
          [name],
        )
        .and_then(|_| wish.execute("DELETE FROM wish_list_keys WHERE name=?1", [name]))
        .and_then(|_| wish.execute("DELETE FROM wish_lists WHERE name=?1", [name]))
        .map_err(failed)?;
    }
  }
  Ok(())
}

/// The entry lists a session record names: its queue, and the list of each of its generations.
/// None when the record names no generations list that exists.
fn named_lists(wish: &Connection, record: &Value) -> Result<Option<HashSet<String>>, String> {
  let (Some(queue), Some(generations)) = (record["queue"].as_str(), record["generations"].as_str())
  else {
    return Ok(None);
  };
  let exists: bool = wish
    .query_row("SELECT EXISTS(SELECT 1 FROM wish_lists WHERE name=?1)", [generations], |row| {
      row.get(0)
    })
    .map_err(|error| error.to_string())?;
  if !exists {
    return Ok(None);
  }
  let values = wish
    .prepare(
      "SELECT value FROM wish_items WHERE list=(SELECT id FROM wish_list_keys WHERE name=?1)",
    )
    .and_then(|mut statement| {
      statement
        .query_map([generations], |row| row.get::<_, Vec<u8>>(0))?
        .collect::<Result<Vec<_>, _>>()
    })
    .map_err(|error| error.to_string())?;
  let mut named = HashSet::from([queue.to_owned()]);
  for value in values {
    let generation: Value = serde_json::from_slice(&value).map_err(|error| error.to_string())?;
    let Some(entries) = generation["entries"].as_str() else {
      return Err("a generation names no entry list".into());
    };
    named.insert(entries.to_owned());
  }
  Ok(Some(named))
}
