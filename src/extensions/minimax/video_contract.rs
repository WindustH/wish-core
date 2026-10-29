//! Supplier-owned video model capabilities and input policy.
use crate::extensions::shared::video::{InputRole::*, VideoCapabilities, VideoMode::*};
pub fn capabilities(_model: &str) -> Option<VideoCapabilities> {
  let (modes, roles, blobs, notes) = {
    (
      vec![Text, Frames, Reference],
      vec![FirstFrame, LastFrame, ReferenceImage, ReferenceVideo, ReferenceAudio],
      vec!["image", "video", "audio"],
      "Up to 9 reference images, 3 videos and 3 audio clips. Frames and references cannot mix. Video/audio total duration at most 15s each; upstream validates durations/codecs.",
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
  crate::extensions::shared::video::validate_sources(&caps, mode, inputs, false)?;
  let count = |role| inputs.iter().filter(|i| i.role == role).count();
  match mode {
    Text if !inputs.is_empty() => return Err(invalid("text mode does not accept inputs")),
    Frames => {
      if inputs.is_empty() || inputs.iter().any(|i| !matches!(i.role, FirstFrame | LastFrame)) {
        return Err(invalid("frames mode requires first_frame and/or last_frame only"));
      }
      if count(FirstFrame) == 0 {
        return Err(invalid("this extension requires first_frame with last_frame"));
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

pub fn validate_input_url(value: &str) -> Result<(), String> {
  crate::extensions::shared::urls::validate_asset_url(value, Some("mm_file://"))
}
