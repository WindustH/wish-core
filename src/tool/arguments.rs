//! Safe omission rules shared by built-in tools and their command adapters.
//! Only schema-declared optional fields are omitted. Required values and payload bytes
//! remain untouched; operation-specific validation still owns their meaning.
use serde_json::Value;

pub fn normalize(arguments: &Value, schema: &Value) -> Result<Value, String> {
  let mut value = if empty(arguments) { serde_json::json!({}) } else { arguments.clone() };
  if !value.is_object() {
    return Err("arguments must be a JSON object, for example {}".into());
  }
  omit_optional(&mut value, schema);
  check_value_types(&value, schema, "arguments")?;
  Ok(value)
}
fn empty(value: &Value) -> bool {
  value.is_null() || value.as_str().is_some_and(|text| text.trim().is_empty())
}
fn omit_optional(value: &mut Value, schema: &Value) {
  if let (Some(object), Some(properties)) =
    (value.as_object_mut(), schema["properties"].as_object())
  {
    for (name, field_schema) in properties {
      let required = schema["required"]
        .as_array()
        .is_some_and(|fields| fields.iter().any(|field| field == name));
      if !required
        && object.get(name).is_some_and(|value| {
          value.is_null() || (field_schema["x-preserve-whitespace"] != true && empty(value))
        })
      {
        object.remove(name);
      } else if let Some(field) = object.get_mut(name) {
        omit_optional(field, field_schema);
      }
    }
  } else if let Some(items) = value.as_array_mut() {
    for item in items {
      omit_optional(item, &schema["items"]);
    }
  }
}
// Check scalar/array types with paths before serde erases the location of nested errors.
// Object interpretation stays with the tool parser (e.g. Ask User also accepts string choices).
fn check_value_types(value: &Value, schema: &Value, path: &str) -> Result<(), String> {
  let expected = schema["type"].as_str().unwrap_or("");
  let valid = match expected {
    "string" => value.is_string(),
    "boolean" => value.is_boolean(),
    "number" => value.is_number(),
    "integer" => value.is_i64() || value.is_u64(),
    "array" => value.is_array(),
    _ => true,
  };
  if !valid {
    return Err(format!("`{path}` must be {expected}"));
  }
  if let (Some(object), Some(properties)) = (value.as_object(), schema["properties"].as_object()) {
    for (name, field) in object {
      if let Some(field_schema) = properties.get(name) {
        check_value_types(field, field_schema, &format!("{path}.{name}"))?;
      }
    }
  } else if let Some(items) = value.as_array() {
    for (index, item) in items.iter().enumerate() {
      check_value_types(item, &schema["items"], &format!("{path}[{index}]"))?;
    }
  }
  Ok(())
}
pub fn error(tool: &str, reason: impl std::fmt::Display, example: &str) -> String {
  format!(
    "{tool}: invalid arguments: {reason}. Correct the indicated field before retrying. Example: {example}"
  )
}
