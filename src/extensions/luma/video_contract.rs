//! Supplier-owned video model capabilities and input policy.
use crate::extensions::shared::video::{InputRole::*, VideoCapabilities, VideoMode::*};
pub fn capabilities(_model: &str) -> Option<VideoCapabilities> {
  let (modes, roles, blobs, notes) = {
    (
      vec![Text, Frames, Edit, Extend],
      vec![FirstFrame, LastFrame, SourceVideo],
      vec![],
      "Inputs require public HTTPS URLs (generated video blobs can reuse recorded URLs). Extend requires a completed Luma job_id from this provider. Modify uses flex_1 by default.",
    )
  };
  Some(VideoCapabilities {
    modes,
    input_roles: roles,
    blob_inputs: blobs.into_iter().map(str::to_owned).collect(),
    notes: notes.into(),
  })
}

pub fn validate(
  model: &str,
  mode: crate::extensions::shared::video::VideoMode,
  inputs: &[crate::extensions::shared::video::VideoInput],
) -> Result<(), crate::protocol::error::Error> {
  let invalid = |text: &str| crate::protocol::error::Error::Build(text.into());
  let caps = capabilities(model).ok_or_else(|| invalid("unsupported video model"))?;
  crate::extensions::shared::video::validate_sources(&caps, mode, inputs, true)?;
  let count = |role| inputs.iter().filter(|i| i.role == role).count();
  match mode {
    Text if !inputs.is_empty() => return Err(invalid("text mode does not accept inputs")),
    Frames => {
      if inputs.is_empty() || inputs.iter().any(|i| !matches!(i.role, FirstFrame | LastFrame)) {
        return Err(invalid("frames mode requires first_frame and/or last_frame only"));
      }
    }
    Reference => {
      if inputs.is_empty()
        || inputs
          .iter()
          .any(|i| !matches!(i.role, ReferenceImage | ReferenceVideo | ReferenceAudio))
      {
        return Err(invalid("reference mode accepts image, video and audio references only"));
      }
    }
    Edit | Extend => {
      if count(SourceVideo) != 1 {
        return Err(invalid("edit/extend requires exactly one source_video"));
      }
      if mode == Extend && !inputs.iter().any(|i| i.role == SourceVideo && i.job_id.is_some()) {
        return Err(invalid("Luma extend requires a completed job_id"));
      }
      if mode == Edit && count(LastFrame) > 0 || mode == Extend && count(FirstFrame) > 0 {
        return Err(invalid(
          "Luma modify accepts first_frame; forward extension accepts last_frame",
        ));
      }
    }
    _ => {}
  }
  if count(ReferenceImage) > 9
    || count(ReferenceVideo) + count(SourceVideo) > 3
    || count(ReferenceAudio) > 3
  {
    return Err(invalid("reference count exceeds provider limits"));
  }
  Ok(())
}

/// Resolve this extension's defaults according to the requested operation.
pub fn parameters(
  defaults: &crate::extensions::shared::parameters::Parameters,
  overrides: &crate::extensions::shared::parameters::Parameters,
  mode: crate::extensions::shared::video::VideoMode,
) -> Result<crate::extensions::shared::parameters::Parameters, crate::protocol::error::Error> {
  use crate::extensions::shared::parameters::{Parameters, overlay};
  let mut result = overlay(defaults, overrides);
  if mode == Edit {
    result = overlay(&Parameters::new(), overrides);
  }
  Ok(result)
}
