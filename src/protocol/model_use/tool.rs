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
  match content {
    Value::String(text) => text.clone(),
    other => other.to_string(),
  }
}
