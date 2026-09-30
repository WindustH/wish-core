//! 1 to 2: sessions gain a `web_search` switch. New sessions start with it on
//! (`defaults.tools.web_search`); every existing session's record in `management.sqlite` gets it off,
//! so its tool list - and the prompt cache keyed on it - stays as it was. A configuration with a
//! ChatGPT (Codex) model provider and no search providers yet gets one search provider borrowing
//! that account, first in the search order.

use super::Data;
use serde_json::{Value, json};

pub const SUMMARY: &str =
  "add the web search switch, on for new sessions and off for existing ones";

pub fn apply(data: &mut Data) -> Result<(), String> {
  if let Some(tools) = data.config.pointer_mut("/defaults/tools").and_then(Value::as_object_mut) {
    tools.entry("web_search").or_insert(Value::Bool(true));
  }
  add_codex_search(data.config);
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
    let Some(tools) = value.pointer_mut("/session/tools").and_then(Value::as_object_mut) else {
      return Err(format!("session {id}: the record has no tool switches"));
    };
    tools.entry("web_search").or_insert(Value::Bool(false));
    let record = serde_json::to_string(&value).map_err(|error| error.to_string())?;
    management
      .execute("UPDATE sessions SET record = ?1 WHERE id = ?2", [&record, &id])
      .map_err(|error| format!("session {id}: {error}"))?;
  }
  Ok(())
}

fn add_codex_search(config: &mut Value) {
  if config.get("search").is_some() {
    return;
  }
  let codex = config
    .get("providers")
    .and_then(Value::as_object)
    .and_then(|providers| {
      providers.iter().find(|(_, provider)| provider["preset"] == "openai_codex")
    })
    .map(|(id, _)| id.clone());
  let Some(lender) = codex else { return };
  let Some(root) = config.as_object_mut() else { return };
  root.insert(
    "search".into(),
    json!({
      "order": ["chatgpt"],
      "providers": {"chatgpt": {"preset": "openai_codex_search", "auth_provider": lender}},
    }),
  );
}
