//! Supplier-owned video model capabilities and input policy.
use crate::extensions::shared::video::{InputRole::*, VideoCapabilities, VideoMode::*};
pub fn capabilities(model: &str) -> Option<VideoCapabilities> {
  let (modes, roles, blobs, notes) = if model == "aleph2" {
    (
      vec![Edit],
      vec![SourceVideo, FirstFrame, LastFrame],
      vec!["image", "video"],
      "Aleph 2 edits one video (up to 30s) with optional first/last guidance frames. Does not accept generation duration/resolution; input framing is preserved.",
    )
  } else if model == "gen4.5" {
    (
      vec![Text, Frames],
      vec![FirstFrame],
      vec!["image"],
      "Gen-4.5 supports a first frame only. Use an Aleph 2 service for editing.",
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

pub fn validate_input_url(value: &str) -> Result<(), String> {
  crate::extensions::shared::urls::validate_asset_url(value, Some("runway://"))
}
