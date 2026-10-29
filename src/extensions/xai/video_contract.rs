//! xAI video input combinations.
use crate::extensions::shared::video::{InputRole::*, VideoMode::*};

pub fn validate(
  model: &str,
  mode: crate::extensions::shared::video::VideoMode,
  inputs: &[crate::extensions::shared::video::VideoInput],
) -> Result<(), crate::protocol::error::Error> {
  let invalid = |text: &str| crate::protocol::error::Error::Build(text.into());
  let caps =
    super::video_wire::capabilities(model).ok_or_else(|| invalid("unsupported video model"))?;
  crate::extensions::shared::video::validate_sources(&caps, mode, inputs, false)?;
  let count = |role| inputs.iter().filter(|i| i.role == role).count();
  let modern = model == "grok-imagine-video-1.5";
  let valid = match mode {
    Text => inputs.is_empty(),
    Frames => {
      !inputs.is_empty()
        && inputs.iter().all(|i| matches!(i.role, FirstFrame | LastFrame | Keyframe))
    }
    Reference => {
      !inputs.is_empty()
        && inputs.iter().all(|i| {
          matches!(i.role, ReferenceImage | ReferenceVoice)
            || (modern && matches!(i.role, FirstFrame | LastFrame | Keyframe))
        })
    }
    Edit | Extend => inputs.len() == 1 && count(SourceVideo) == 1,
  };
  if !valid
    || count(Keyframe) > 4
    || count(ReferenceVoice) > 3
    || count(ReferenceImage) > if modern { 14 } else { 7 }
  {
    return Err(invalid("unsupported xAI video input combination/count"));
  }
  Ok(())
}

/// Resolve this extension's defaults according to the requested operation.
pub fn parameters(
  defaults: &crate::extensions::shared::parameters::Parameters,
  overrides: &crate::extensions::shared::parameters::Parameters,
  mode: crate::extensions::shared::video::VideoMode,
  inputs: &[crate::extensions::shared::video::VideoInput],
) -> Result<crate::extensions::shared::parameters::Parameters, crate::protocol::error::Error> {
  use crate::extensions::shared::parameters::{Parameters, overlay};
  let mut result = overlay(defaults, overrides);
  if !overrides.get("aspect_ratio").is_some_and(|v| !v.is_null())
    && inputs.len() == 1
    && inputs[0].role == FirstFrame
  {
    result.remove("aspect_ratio");
  }
  if matches!(mode, Edit | Extend) {
    result = overlay(&Parameters::new(), overrides);
  }
  Ok(result)
}
