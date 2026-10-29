//! Provider-owned video configuration.
#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct VideoOptions {
  pub duration: Option<f64>,
  pub aspect_ratio: Option<String>,
  pub resolution: Option<String>,
  pub duration_hint: Option<String>,
  pub language: Option<String>,
  pub watermark: Option<bool>,
}
