//! Supplier-owned request validation and wire encoding.
use crate::protocol::error::Error;
use serde_json::{Value, json};
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
  pub duration: Option<f64>,
  pub voice: Option<String>,
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
    if options.duration.is_some() {
      return Err(invalid("speech duration is determined by the text"));
    }
    if prompt.chars().count() > 4096 {
      return Err(invalid("speech input exceeds 4096 characters"));
    }
    let voice = options
      .voice
      .as_deref()
      .filter(|v| !v.trim().is_empty() && v.len() <= 200)
      .ok_or_else(|| invalid("speech requires a configured or requested voice ID"))?;
    (
      "/v1/audio/speech".into(),
      json!({"model":model,"input":prompt,"voice":voice,"response_format":"mp3"}),
    )
  })
}

/// Check service defaults without requiring a voice until invocation time.
pub fn validate_configuration(
  model: &str,
  values: &crate::extensions::shared::parameters::Parameters,
) -> Result<(), Error> {
  let mut options: Options = crate::extensions::shared::parameters::decode(values)?;
  options.voice.get_or_insert_with(|| "validation".into());
  submission(model, "validation", &options).map(|_| ())
}
