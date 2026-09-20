use serde_json::Value;

/// One tool the model may call: a name, a description and the schema for its arguments.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct Tool {
  pub name: String,
  pub description: String,
  /// A JSON Schema object describing the arguments a call takes.
  pub input_schema: Value,
}
