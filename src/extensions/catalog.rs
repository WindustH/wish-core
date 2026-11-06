//! Extension-owned tool registration; no common business request or result schema.
use crate::protocol::Tool;
use serde_json::Value;

pub const DISCOVERY_TOOL: &str = "extension_tools";
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Permission {
  ImageGeneration,
  VideoGeneration,
  AudioGeneration,
  Transcription,
  WebSearch,
  WebFetch,
}
pub use super::invocation::Invocation;
#[derive(Clone, Copy)]
pub struct Registration {
  pub extension: &'static str,
  pub name: &'static str,
  pub summary: &'static str,
  pub tags: &'static [&'static str],
  pub service_presets: &'static [&'static str],
  /// Whether the supplier accepts a per-invocation model ID.
  pub model_override: bool,
  pub permission: Permission,
  pub specification: fn() -> Tool,
  pub examples: fn() -> Value,
  pub parse: fn(Value) -> Result<Invocation, String>,
}
impl Registration {
  pub fn tool(&self) -> Tool {
    let mut tool = (self.specification)();
    if self.model_override {
      tool.input_schema["properties"]["model"] = serde_json::json!({
        "type":"string", "minLength":1, "maxLength":256,
        "description":"Optional model ID for this invocation only. Overrides the selected service default without changing saved settings. Sent unchanged; upstream determines availability and access."
      });
    }
    tool
  }
}
/// Only structural checks: model IDs are owned by upstream services.
pub fn validate_model(model: &str) -> Result<(), String> {
  if model.trim().is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
    Err("model must be a nonempty string of at most 256 bytes without control characters".into())
  } else {
    Ok(())
  }
}
pub fn registrations() -> &'static [Registration] {
  static ITEMS: std::sync::OnceLock<Vec<Registration>> = std::sync::OnceLock::new();
  ITEMS.get_or_init(|| {
    super::package::packages()
      .iter()
      .flat_map(|p| {
        p.tools.iter().map(|r| {
          assert_eq!(r.extension, p.id, "extension tool owner");
          **r
        })
      })
      .collect()
  })
}

pub fn find(name: &str) -> Option<&'static Registration> {
  registrations().iter().find(|registration| registration.name == name)
}
pub fn is_tool(name: &str) -> bool {
  name == DISCOVERY_TOOL || find(name).is_some()
}
