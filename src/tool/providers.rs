//! Stable provider selection and live discovery for service-backed tools.
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub struct Provider {
  pub id: String,
  pub name: String,
  pub preset: String,
  pub model: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub kind: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub mask_editing: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub markup_editing: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub image_required: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub reference_images: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub precise_references: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub image_options: Option<&'static [&'static str]>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub video: Option<crate::extension_host::media::video::VideoCapabilities>,
  pub enabled: bool,
  /// Local configuration/credential readiness, not a remote health or quota check.
  pub available: bool,
}
#[derive(Serialize)]
pub struct Catalog {
  pub default_order: Vec<String>,
  pub providers: Vec<Provider>,
  pub failure_policy: &'static str,
}
pub type Listing = Arc<dyn Fn() -> Result<Catalog, String> + Send + Sync>;
