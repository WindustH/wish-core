//! Provider-independent image invocation inputs shared by tools, hosts and extensions.
use super::images::ReferenceKind;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRequest {
  pub prompt: String,
  #[serde(default)]
  pub options: serde_json::Map<String, Value>,
  #[serde(default)]
  pub images: Vec<String>,
  #[serde(default)]
  pub mask: Option<String>,
  #[serde(default)]
  pub markup: Option<String>,
  #[serde(default)]
  pub references: Vec<PreciseReferenceRequest>,
  #[serde(default)]
  pub provider: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreciseReferenceRequest {
  pub image: String,
  pub kind: ReferenceKind,
  #[serde(default = "reference_default")]
  pub strength: f64,
  #[serde(default = "reference_default")]
  pub fidelity: f64,
}
fn reference_default() -> f64 {
  1.0
}
