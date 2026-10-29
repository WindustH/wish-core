//! Supplier-owned video model capabilities and input policy.
use crate::extensions::shared::video::{InputRole::*, VideoCapabilities, VideoMode::*};
pub fn capabilities(_model: &str, version25: bool) -> Option<VideoCapabilities> {
  let (modes, roles, blobs, notes) = {
    (
      if version25 {
        vec![Text, Frames, Reference, Edit, Extend]
      } else {
        vec![Text, Frames, Reference]
      },
      vec![FirstFrame, LastFrame, ReferenceImage, ReferenceVideo, ReferenceAudio, SourceVideo],
      vec!["image", "audio"],
      "Video requires HTTPS/asset URL; a generated video blob can reuse its recorded upstream URL. Frame inputs cannot mix with references. Seedance 2.5 supports edit/extend; edit fixes duration=-1 and ratio=adaptive. Upstream enforces media dimensions/duration/codec limits.",
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
  version25: bool,
) -> Result<(), crate::protocol::error::Error> {
  let invalid = |text: &str| crate::protocol::error::Error::Build(text.into());
  let caps = capabilities(model, version25).ok_or_else(|| invalid("unsupported video model"))?;
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
    Edit | Extend => {
      if count(SourceVideo) != 1 {
        return Err(invalid("edit/extend requires exactly one source_video"));
      }
      if inputs.iter().any(|i| matches!(i.role, FirstFrame | LastFrame)) {
        return Err(invalid("Seedance edit/extend uses references, not frame roles"));
      }
    }
    _ => {}
  }
  let large = version25;
  if count(ReferenceImage) > if large { 30 } else { 9 }
    || count(ReferenceVideo) + count(SourceVideo) > if large { 10 } else { 3 }
    || count(ReferenceAudio) > if large { 10 } else { 3 }
  {
    return Err(invalid("reference count exceeds provider limits"));
  }
  if !version25 && count(ReferenceAudio) > 0 && count(ReferenceImage) + count(ReferenceVideo) == 0 {
    return Err(invalid("Seedance 2.0 audio references require an image or video"));
  }
  Ok(())
}

/// Resolve this extension's defaults according to the requested operation.
pub fn parameters(
  defaults: &crate::extensions::shared::parameters::Parameters,
  overrides: &crate::extensions::shared::parameters::Parameters,
  mode: crate::extensions::shared::video::VideoMode,
  version25: bool,
) -> Result<crate::extensions::shared::parameters::Parameters, crate::protocol::error::Error> {
  use crate::extensions::shared::parameters::overlay;
  let mut result = overlay(defaults, overrides);
  if version25 && matches!(mode, Edit | Extend) {
    if overrides.get("aspect_ratio").is_some_and(|v| !v.is_null() && v != "adaptive") {
      return Err(crate::protocol::error::Error::Build(
        "Seedance edit/extend requires adaptive aspect_ratio".into(),
      ));
    }
    result.insert("aspect_ratio".into(), serde_json::json!("adaptive"));
    if mode == Edit {
      if overrides.get("duration").is_some_and(|v| !v.is_null() && v.as_f64() != Some(-1.0)) {
        return Err(crate::protocol::error::Error::Build(
          "Seedance editing preserves duration; omit duration or use -1".into(),
        ));
      }
      result.insert("duration".into(), serde_json::json!(-1.0));
    }
  }
  Ok(result)
}

pub fn validate_input_url(value: &str) -> Result<(), String> {
  crate::extensions::shared::urls::validate_asset_url(value, Some("asset://"))
}
