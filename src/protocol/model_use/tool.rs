//! What the model may call, and the two shapes a call's data travels in on every wire.

use serde_json::Value;

/// One tool the model may call: a name, a description and the schema for its arguments.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Tool {
  pub name: String,
  pub description: String,
  /// A JSON Schema object describing the arguments a call takes.
  pub input_schema: Value,
}

/// A call's arguments as the wire spelled them, a JSON text: a blank one is a call without
/// arguments, an empty object.
pub(crate) fn parse_tool_arguments(raw: &str) -> serde_json::Result<Value> {
  if raw.trim().is_empty() {
    Ok(Value::Object(Default::default()))
  } else {
    serde_json::from_str(raw)
  }
}

/// A tool result as the text the wires that take a string carry: a string payload passes through,
/// anything else travels as its JSON.
pub(crate) fn tool_result_text(content: &Value) -> String {
  if let Some(text) = content.get("model_text").and_then(Value::as_str) {
    return text.to_owned();
  }
  match content {
    Value::String(text) => text.clone(),
    other => other.to_string(),
  }
}

/// Only new reports opt into presentation projection; historical payloads stay unchanged.
pub(crate) fn tool_result_value(content: &Value) -> Value {
  if let Some(text) = content.get("model_text").and_then(Value::as_str) {
    serde_json::json!({"status":content["status"], "content":text})
  } else {
    content.clone()
  }
}
pub(crate) fn tool_result_is_error(content: &Value) -> bool {
  matches!(content["status"].as_str(), Some("failed" | "cancelled" | "unknown"))
}

#[cfg(test)]
mod result_tests {
  use super::*;
  use serde_json::json;
  #[test]
  fn report_projection_hides_process_data_and_preserves_old_results() {
    let content = json!({"status":"failed", "model_text":"Command failed (exit code 1).\nerror", "output":{"encoding":"utf8", "process":{"exit_code":1}, "output_path":"private"}});
    assert_eq!(tool_result_text(&content), "Command failed (exit code 1).\nerror");
    let projected = tool_result_value(&content).to_string();
    assert!(!projected.contains("encoding") && !projected.contains("private"));
    assert!(tool_result_is_error(&content));
    let old = json!({"status":"success", "output":{"text":"hello"}});
    assert_eq!(tool_result_value(&old), old);
    assert_eq!(tool_result_text(&old), old.to_string());
  }
}
