//! Supplier-owned image geometry and editing semantics.
use crate::extensions::shared::image_pixels::decode;
use crate::extensions::shared::images::{ReferenceImage, ReferenceInput};
pub(crate) fn validate(
  images: &[ReferenceInput],
  mask: Option<&ReferenceImage>,
) -> Result<(), String> {
  let [ReferenceInput::Inline(source)] = images else {
    return Err("Fill requires one local image blob".into());
  };
  let source = decode(source, false)?;
  if let Some(mask) = mask {
    if mask.mime != "image/png" {
      return Err("mask must be PNG".into());
    }
    let mask = decode(mask, false)?;
    if source.width() != mask.width() || source.height() != mask.height() {
      return Err("mask dimensions must match the source image".into());
    }
    let mask = mask.to_rgba8();
    if mask
      .pixels()
      .any(|p| p[3] != 255 || !matches!(p[0], 0 | 255) || p[1] != p[0] || p[2] != p[0])
    {
      return Err("mask must be opaque black and white (white edits, black preserves)".into());
    }
    if !mask.pixels().any(|p| p[0] == 255) {
      return Err("mask does not select any pixels to edit".into());
    }
  } else if !source.to_rgba8().pixels().any(|p| p[3] < 255) {
    return Err("Fill requires a mask or a source image with transparent pixels".into());
  }
  Ok(())
}
