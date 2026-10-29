//! Bounded pixel decoding/encoding; suppliers decide geometry and overlay semantics.
use super::images::ReferenceImage;
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use std::io::Cursor;
pub(crate) fn decode(image: &ReferenceImage, oriented: bool) -> Result<DynamicImage, String> {
  let bytes = STANDARD.decode(&image.data).map_err(|e| e.to_string())?;
  let mut reader =
    ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
  let mut limits = image::Limits::default();
  limits.max_image_width = Some(4096);
  limits.max_image_height = Some(4096);
  limits.max_alloc = Some(128 * 1024 * 1024);
  reader.limits(limits);
  let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
  let orientation = decoder.orientation().map_err(|e| e.to_string())?;
  let mut decoded =
    DynamicImage::from_decoder(decoder).map_err(|e| format!("invalid mask/source image: {e}"))?;
  if oriented {
    decoded.apply_orientation(orientation);
  }
  Ok(decoded)
}
pub(crate) fn encode(image: DynamicImage, max_bytes: usize) -> Result<ReferenceImage, String> {
  let mut bytes = Cursor::new(Vec::new());
  image.write_to(&mut bytes, ImageFormat::Png).map_err(|e| e.to_string())?;
  if bytes.get_ref().len() >= max_bytes {
    return Err("prepared edit image exceeds size limit".into());
  }
  Ok(ReferenceImage { mime: "image/png".into(), data: STANDARD.encode(bytes.into_inner()) })
}
