//! Provider-owned video configuration.
#[derive(Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct VideoOptions {
  pub generate_audio: Option<bool>,
  pub duration: Option<f64>,
  pub aspect_ratio: Option<String>,
  pub resolution: Option<String>,
}
