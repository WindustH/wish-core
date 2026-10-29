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
  pub accepts_model: fn(&str) -> bool,
  pub permission: Permission,
  pub specification: fn() -> Tool,
  pub examples: fn() -> Value,
  pub parse: fn(Value) -> Result<Invocation, String>,
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
