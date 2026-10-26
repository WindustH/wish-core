//! New sessions discover history through Wish; stored sessions keep their existing direct tools.
use super::Data;
use serde_json::json;
pub const SUMMARY: &str = "add configurable history access for new sessions";
pub fn apply(data: &mut Data) -> Result<(), String> {
  if let Some(tools) =
    data.config.pointer_mut("/defaults/tools").and_then(|value| value.as_object_mut())
  {
    tools.entry("history_search").or_insert(json!(true));
    tools.entry("history_search_delivery").or_insert(json!("command"));
  }
  Ok(())
}
