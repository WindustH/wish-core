//! Application-owned endpoint presets, independent of credentials and protocol rendering.
use serde_json::Value;
use std::sync::OnceLock;
pub fn catalog() -> &'static Value {
  static CATALOG: OnceLock<Value> = OnceLock::new();
  CATALOG.get_or_init(|| {
    serde_json::from_str(include_str!("../../resources/provider-presets.json"))
      .expect("valid built-in provider presets")
  })
}
pub fn find(id: &str) -> Option<&'static Value> {
  catalog()["presets"].as_array().unwrap().iter().find(|p| p["id"] == id)
}
