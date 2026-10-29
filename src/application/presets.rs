//! The built-in presets, independent of credentials and protocol rendering: provider presets (a
//! service's endpoint, protocol, models and limits) and search presets (a search service's protocol
//! and, for one that comes with a subscription, the provider presets whose account it can borrow).
use serde_json::Value;
use std::sync::OnceLock;

pub fn provider_presets() -> &'static Value {
  static PRESETS: OnceLock<Value> = OnceLock::new();
  PRESETS.get_or_init(|| {
    serde_json::from_str(include_str!("../../resources/provider-presets.json"))
      .expect("valid built-in provider presets")
  })
}
pub fn find_provider(id: &str) -> Option<&'static Value> {
  provider_presets()["presets"].as_array()?.iter().find(|p| p["id"] == id)
}
pub fn search_presets() -> &'static Value {
  static PRESETS: OnceLock<Value> = OnceLock::new();
  PRESETS
    .get_or_init(|| serde_json::json!({"presets":crate::extensions::package::presets("search")}))
}
pub fn find_search(id: &str) -> Option<&'static Value> {
  search_presets()["presets"].as_array()?.iter().find(|p| p["id"] == id)
}

pub fn fetch_presets() -> &'static Value {
  static PRESETS: OnceLock<Value> = OnceLock::new();
  PRESETS
    .get_or_init(|| serde_json::json!({"presets":crate::extensions::package::presets("fetch")}))
}
pub fn find_fetch(id: &str) -> Option<&'static Value> {
  fetch_presets()["presets"].as_array()?.iter().find(|p| p["id"] == id)
}

pub fn renewal(preset: Option<&str>) -> Option<crate::protocol::endpoint::CredentialRenewal> {
  use crate::protocol::endpoint::CredentialRenewal;
  match preset {
    Some("openai_codex") => Some(CredentialRenewal::CodexOAuth),
    Some("github_copilot") => Some(CredentialRenewal::CopilotToken),
    _ => None,
  }
}
