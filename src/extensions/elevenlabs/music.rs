//! Supplier-owned request validation and wire encoding.
use crate::protocol::error::Error;
use serde_json::{Value, json};
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
  pub duration: Option<f64>,
  pub instrumental: Option<bool>,
}
pub(crate) fn submission(
  model: &str,
  prompt: &str,
  options: &Options,
) -> Result<(String, Value), Error> {
  let invalid = |s: &str| Error::Build(s.into());
  if prompt.trim().is_empty() || prompt.chars().count() > 4100 {
    return Err(invalid("prompt is empty or exceeds the service limit"));
  }
  Ok({
    let mut body = json!({"prompt":prompt,"model_id":model,"force_instrumental":options.instrumental.unwrap_or(false)});
    if let Some(duration) = options.duration {
      if !(3.0..=600.0).contains(&duration) {
        return Err(invalid("music duration must be 3..600 seconds"));
      }
      body["music_length_ms"] = json!((duration * 1000.0).round() as u64);
    }
    ("/v1/music?output_format=mp3_44100_128".into(), body)
  })
}
