//! 4 to 5: a session's record keeps its last operation's outcome in the short form the session
//! list shows - why a stream failed, why the model stopped - and not the partial or whole response
//! behind it. Records saved before the server trimmed outcomes still hold the response.

use super::Data;
use serde_json::{Map, Value};

pub const SUMMARY: &str = "keep only the short form of each session's last outcome in its record";

pub fn apply(data: &mut Data) -> Result<(), String> {
  let Some(management) = data.management else { return Ok(()) };
  let records = management
    .prepare("SELECT id, record FROM sessions")
    .and_then(|mut statement| {
      statement
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()
    })
    .map_err(|error| error.to_string())?;
  for (id, record) in records {
    let mut value: Value =
      serde_json::from_str(&record).map_err(|error| format!("session {id}: {error}"))?;
    let Some(outcome) = value.pointer_mut("/status/last_operation/outcome") else { continue };
    let before = outcome.clone();
    for (variant, field) in [("StreamFailed", "reason"), ("ModelStopped", "stop_reason")] {
      if let Some(detail) = outcome.get_mut(variant) {
        let kept = detail.get(field).cloned().unwrap_or(Value::Null);
        *detail = Value::Object(Map::from_iter([(field.to_owned(), kept)]));
      }
    }
    if *outcome == before {
      continue;
    }
    let record = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    management
      .execute("UPDATE sessions SET record = ?1 WHERE id = ?2", [&record, &id])
      .map_err(|error| format!("session {id}: {error}"))?;
  }
  Ok(())
}
