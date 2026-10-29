//! Supplier-owned request validation and wire encoding.
use crate::protocol::error::Error;
use serde_json::{Value, json};
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
  pub duration: Option<f64>,
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
    let mut body = json!({"text":prompt,"model_id":model});
    if let Some(duration) = options.duration {
      if !(0.5..=30.0).contains(&duration) {
        return Err(invalid("sound effect duration must be 0.5..30 seconds"));
      }
      body["duration_seconds"] = json!(duration);
    }
    ("/v1/sound-generation?output_format=mp3_44100_128".into(), body)
  })
}
