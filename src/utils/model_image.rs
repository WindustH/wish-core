//! Bounded image normalization before model payload encoding. Originals are never overwritten.
use image::{ImageDecoder, ImageEncoder};
use sha2::{Digest, Sha256};
use std::{
  collections::VecDeque,
  io::Cursor,
  sync::{Arc, LazyLock, Mutex},
};

pub const MAX_SOURCE_BYTES: usize = 100 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_EDGE: u32 = 2048;
pub struct Prepared {
  pub bytes: Vec<u8>,
  pub mime: &'static str,
}
type Cache = VecDeque<([u8; 32], Arc<Prepared>)>;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(VecDeque::new()));

pub fn prepare(bytes: &[u8]) -> Result<Arc<Prepared>, String> {
  if bytes.len() > MAX_SOURCE_BYTES {
    return Err("image exceeds 100 MiB".into());
  }
  let key: [u8; 32] = Sha256::digest(bytes).into();
  // Decoder scratch memory is bounded process-wide, including legacy history projection.
  static DECODER: Mutex<()> = Mutex::new(());
  let _decoder = DECODER.lock().unwrap_or_else(|error| error.into_inner());
  {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(index) = cache.iter().position(|(id, _)| id == &key) {
      let entry = cache.remove(index).expect("existing cache entry");
      let result = entry.1.clone();
      cache.push_back(entry);
      return Ok(result);
    }
  }
  let result = Arc::new(convert(bytes)?);
  let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
  cache.retain(|(id, _)| id != &key);
  cache.push_back((key, result.clone()));
  while cache.len() > 32
    || cache.iter().map(|(_, image)| image.bytes.len()).sum::<usize>() > 32 * 1024 * 1024
  {
    cache.pop_front();
  }
  Ok(result)
}
fn convert(bytes: &[u8]) -> Result<Prepared, String> {
  let decode = || -> Result<Prepared, Box<dyn std::error::Error>> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let format = reader.format();
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(256 * 1024 * 1024);
    limits.max_image_width = Some(32768);
    limits.max_image_height = Some(32768);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    if decoder.total_bytes() > 256 * 1024 * 1024 {
      return Err("decoded image exceeds 256 MiB".into());
    }
    let orientation = decoder.orientation()?;
    let mut picture = image::DynamicImage::from_decoder(decoder)?;
    picture.apply_orientation(orientation);
    if picture.width().max(picture.height()) <= MAX_EDGE
      && bytes.len() <= MAX_OUTPUT_BYTES
      && orientation == image::metadata::Orientation::NoTransforms
      && matches!(format, Some(image::ImageFormat::Png | image::ImageFormat::Jpeg))
    {
      return Ok(Prepared {
        bytes: bytes.to_vec(),
        mime: if format == Some(image::ImageFormat::Png) { "image/png" } else { "image/jpeg" },
      });
    }
    if picture.width().max(picture.height()) > MAX_EDGE {
      picture = picture.thumbnail(MAX_EDGE, MAX_EDGE);
    }
    loop {
      let rgba = picture.to_rgba8();
      let alpha = rgba.pixels().any(|pixel| pixel.0[3] != 255);
      let mut png = Vec::new();
      image::codecs::png::PngEncoder::new(&mut png).write_image(
        rgba.as_raw(),
        rgba.width(),
        rgba.height(),
        image::ExtendedColorType::Rgba8,
      )?;
      // Preserve text, diagrams and transparency losslessly whenever the budget permits.
      if png.len() <= MAX_OUTPUT_BYTES {
        return Ok(Prepared { bytes: png, mime: "image/png" });
      }
      if !alpha {
        let rgb = picture.to_rgb8();
        for quality in [85, 75, 65] {
          let mut jpeg = Vec::new();
          image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, quality).encode(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
          )?;
          if jpeg.len() <= MAX_OUTPUT_BYTES {
            return Ok(Prepared { bytes: jpeg, mime: "image/jpeg" });
          }
        }
      }
      if picture.width().max(picture.height()) <= 128 {
        return Err("cannot fit image into model input budget".into());
      }
      picture =
        picture.thumbnail((picture.width() * 3 / 4).max(1), (picture.height() * 3 / 4).max(1));
    }
  };
  decode().map_err(|error| format!("Cannot prepare image for model input: {error}. Detected {}. Supported decoders: PNG, JPEG, GIF, WebP, BMP, TIFF; animated files use the first frame. For unsupported formats, try converting a copy to PNG, then call view_image again.", super::image_format::unsupported_format(bytes)))
}

/// Admission precedes reading, so concurrent uploads cannot each retain a 100 MiB buffer.
pub async fn prepare_file(path: &std::path::Path) -> Result<Arc<Prepared>, String> {
  static ADMISSION: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));
  let permit = ADMISSION.clone().acquire_owned().await.map_err(|error| error.to_string())?;
  let bytes =
    super::bounded_file::read(path, MAX_SOURCE_BYTES).await.map_err(|error| error.to_string())?;
  super::blocking::cpu(move || {
    let _permit = permit;
    prepare(&bytes)
  })
  .await
  .map_err(|error| error.to_string())?
}
