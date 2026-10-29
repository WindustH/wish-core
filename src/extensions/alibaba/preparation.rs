//! Supplier-owned image geometry and editing semantics.
use crate::extensions::shared::image_pixels::{decode, encode};
use crate::extensions::shared::images::ReferenceInput;
use image::DynamicImage;
/// Preserve geometry for Wan pixel boxes; strip an opaque alpha channel, never flatten transparency silently.
pub(crate) fn prepare_references(
  images: &mut [ReferenceInput],
  parameters: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
  let boxes = super::image::boxes(parameters).map_err(|e| e.to_string())?;
  if boxes.as_ref().is_some_and(|boxes| boxes.len() != images.len()) {
    return Err("bbox_list must have one item per input image".into());
  }
  let mut total = 0;
  for (index, input) in images.iter_mut().enumerate() {
    let ReferenceInput::Inline(image) = input else {
      continue;
    };
    let decoded = decode(image, true)?;
    let (width, height) = (decoded.width(), decoded.height());
    if width < 240 || height < 240 || width > height * 8 || height > width * 8 {
      return Err("Wan local references require edges 240..4096 and aspect ratio 1:8..8:1".into());
    }
    if boxes
      .as_ref()
      .is_some_and(|boxes| boxes[index].iter().any(|[_, _, x2, y2]| *x2 > width || *y2 > height))
    {
      return Err("Wan editing box extends outside its oriented source image".into());
    }
    if decoded.to_rgba8().pixels().any(|p| p[3] != 255) {
      return Err("Wan references must be opaque; remove transparency before generation".into());
    }
    *image = encode(DynamicImage::ImageRgb8(decoded.to_rgb8()), 12 * 1024 * 1024)?;
    total += image.data.len();
    if total > 16 * 1024 * 1024 {
      return Err("prepared Wan references exceed 12 MiB".into());
    }
  }
  Ok(())
}
