//! Bounded local preparation for NovelAI's documented reference-image canvases.
//! Canvas contract: https://image.novelai.net/docs/doc.json (director_reference_images).
//! Runs under the CPU limiter; inputs have already passed session ownership and byte limits.
use super::references::{PreciseReference, ReferenceKind};
use crate::extensions::shared::images::{ReferenceImage, ReferenceInput};
use base64::{Engine, engine::general_purpose::STANDARD};
use image::{ImageDecoder, ImageFormat, ImageReader, Rgb, RgbImage, imageops};
use std::io::Cursor;

pub(crate) fn prepare(
  images: Vec<ReferenceInput>,
  settings: Vec<(ReferenceKind, f64, f64)>,
) -> Result<Vec<PreciseReference>, String> {
  let mut total = 0;
  images
    .into_iter()
    .zip(settings)
    .map(|(input, (kind, strength, fidelity))| {
      let ReferenceInput::Inline(input) = input else {
        return Err("precise references require session image blobs".into());
      };
      let bytes = STANDARD.decode(input.data).map_err(|e| e.to_string())?;
      let mut reader =
        ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
      let mut limits = image::Limits::default();
      limits.max_image_width = Some(4096);
      limits.max_image_height = Some(4096);
      limits.max_alloc = Some(128 * 1024 * 1024);
      reader.limits(limits);
      let mut decoder =
        reader.into_decoder().map_err(|e| format!("invalid reference image: {e}"))?;
      let orientation = decoder.orientation().map_err(|e| e.to_string())?;
      let mut decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| format!("invalid reference image: {e}"))?;
      decoded.apply_orientation(orientation);
      let (width, height) = (decoded.width(), decoded.height());
      if width == 0 || height == 0 {
        return Err("reference image is empty".into());
      }
      // Pick the permitted canvas that retains the largest fraction of image content.
      let (target_width, target_height) = [(1024u32, 1536u32), (1472, 1472), (1536, 1024)]
        .into_iter()
        .min_by(|(aw, ah), (bw, bh)| {
          let ratio = width as f64 / height as f64;
          (ratio / (*aw as f64 / *ah as f64))
            .ln()
            .abs()
            .total_cmp(&(ratio / (*bw as f64 / *bh as f64)).ln().abs())
        })
        .ok_or("no reference canvas available")?;
      let scaled =
        decoded.resize(target_width, target_height, imageops::FilterType::Triangle).to_rgba8();
      let mut canvas = RgbImage::from_pixel(target_width, target_height, Rgb([0, 0, 0]));
      let offset_x = (target_width - scaled.width()) / 2;
      let offset_y = (target_height - scaled.height()) / 2;
      // Composite transparency against black, rather than discarding alpha and revealing hidden RGB.
      for (x, y, pixel) in scaled.enumerate_pixels() {
        let alpha = u16::from(pixel[3]);
        canvas.put_pixel(
          offset_x + x,
          offset_y + y,
          Rgb([0, 1, 2].map(|channel| ((u16::from(pixel[channel]) * alpha + 127) / 255) as u8)),
        );
      }
      let mut output = Cursor::new(Vec::new());
      image::DynamicImage::ImageRgb8(canvas)
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|e| e.to_string())?;
      let bytes = output.into_inner();
      total += bytes.len();
      if total > 32 * 1024 * 1024 {
        return Err("normalized references exceed 32 MiB".into());
      }
      Ok(PreciseReference {
        image: ReferenceImage { mime: "image/png".into(), data: STANDARD.encode(bytes) },
        kind,
        strength,
        fidelity,
      })
    })
    .collect()
}
