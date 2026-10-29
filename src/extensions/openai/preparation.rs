//! Supplier-owned image geometry and editing semantics.
use crate::extensions::shared::image_pixels::{decode, encode};
use crate::extensions::shared::images::{EditOverlay, ReferenceInput};
use image::DynamicImage;
/// Normalize paired geometry once, without resizing. Additional references remain untouched.
pub(crate) fn prepare_overlay(
  images: &mut [ReferenceInput],
  overlay: EditOverlay,
) -> Result<EditOverlay, String> {
  let Some(ReferenceInput::Inline(source)) = images.first_mut() else {
    return Err("mask/markup requires the first source to be a local image blob".into());
  };
  let decoded_source = decode(source, true)?;
  if overlay.image().mime != "image/png" {
    return Err("mask/markup must be PNG".into());
  }
  let mut pixels = decode(overlay.image(), true)?.to_rgba8();
  if pixels.width() != decoded_source.width() || pixels.height() != decoded_source.height() {
    return Err("mask/markup dimensions must match the oriented first source".into());
  }
  let is_mask = matches!(overlay, EditOverlay::Mask(_));
  if is_mask {
    if pixels
      .pixels()
      .any(|p| p[3] != 255 || !matches!(p[0], 0 | 255) || p[1] != p[0] || p[2] != p[0])
    {
      return Err("mask must be opaque black and white (white edits, black preserves)".into());
    }
    if !pixels.pixels().any(|p| p[0] == 255) {
      return Err("mask selects no pixels to edit".into());
    }
    for pixel in pixels.pixels_mut() {
      pixel[3] = 255 - pixel[0];
    }
  } else if !pixels.pixels().any(|p| p[3] < 255) || !pixels.pixels().any(|p| p[3] > 0) {
    return Err("markup must contain both visible annotation and transparent pixels".into());
  }
  let prepared = encode(DynamicImage::ImageRgba8(pixels), 4 * 1024 * 1024)?;
  *source = encode(decoded_source, 12 * 1024 * 1024)?;
  Ok(if is_mask { EditOverlay::Mask(prepared) } else { EditOverlay::Markup(prepared) })
}
