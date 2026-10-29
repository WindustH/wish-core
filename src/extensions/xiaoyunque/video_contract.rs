//! Supplier-owned video model capabilities and input policy.
use crate::extensions::shared::video::{InputRole::*, VideoCapabilities, VideoMode::*};
pub fn capabilities(model: &str) -> Option<VideoCapabilities> {
  let (modes, roles, blobs, notes) = if model == "pippit_iv2v_v20_cvtob_with_vinput" {
    (
      vec![Text, Reference],
      vec![ReferenceImage, ReferenceVideo],
      vec![],
      "Xiaoyunque 2.0 agent accepts up to 50 public HTTPS image/video references. Local files need public hosting; generated video blobs may reuse recorded URLs. Optional duration_hint is ～15s, ～30s, 40～60s or 1min+; duration is approximate. Requires Volcengine visual AK/SK credentials.",
    )
  } else {
    return None;
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
  crate::extensions::shared::video::validate_sources(&caps, mode, inputs, false)?;
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
    Edit | Extend if count(SourceVideo) != 1 => {
      return Err(invalid("edit/extend requires exactly one source_video"));
    }
    _ => {}
  }
  Ok(())
}
