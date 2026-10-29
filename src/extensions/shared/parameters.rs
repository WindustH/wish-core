//! JSON storage and override mechanics, with no knowledge of supplier parameter names.
use crate::protocol::error::Error;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
pub type Parameters = Map<String, Value>;
pub fn decode<T: DeserializeOwned>(values: &Parameters) -> Result<T, Error> {
  serde_json::from_value(Value::Object(values.clone())).map_err(|e| Error::Build(e.to_string()))
}
pub fn overlay(defaults: &Parameters, overrides: &Parameters) -> Parameters {
  let mut result = defaults.clone();
  for (key, value) in overrides {
    if !value.is_null() {
      result.insert(key.clone(), value.clone());
    }
  }
  result.retain(|_, value| !value.is_null());
  result
}
