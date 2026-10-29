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
    let voice = options
      .voice
      .as_deref()
      .ok_or_else(|| invalid("speech requires a configured or requested voice ID"))?;
    if voice.is_empty()
      || voice.len() > 100
      || !voice.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
      return Err(invalid("invalid voice ID"));
    }
    (
      format!("/v1/text-to-speech/{voice}?output_format=mp3_44100_128"),
      json!({"text":prompt,"model_id":model}),
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
