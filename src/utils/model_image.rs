//! Bounded image normalization before model payload encoding. Originals are never overwritten.
use image::{ImageDecoder, ImageEncoder};
use sha2::{Digest, Sha256};
use std::{
  collections::VecDeque,
  io::Cursor,
  sync::{Arc, LazyLock, Mutex},
};

pub const MAX_SOURCE_BYTES: usize = 100 * 1024 * 1024;
/// Total Base64 image bytes allowed in one model request.
pub const MAX_WIRE_BYTES: usize = 24 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_EDGE: u32 = 2048;
pub struct Prepared {
  pub dimensions: (u32, u32),
  pub bytes: Vec<u8>,
  pub mime: &'static str,
}
const CACHE_IDLE: std::time::Duration = std::time::Duration::from_secs(10 * 60);
type Cache = VecDeque<([u8; 32], Arc<Prepared>, std::time::Instant)>;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(VecDeque::new()));
static ADMISSION: LazyLock<Arc<tokio::sync::Semaphore>> =
  LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(2)));

/// Inline/legacy images share the file decoder's admission and CPU pool. Admission
/// precedes Base64 decoding so queued requests do not also retain decoded buffers.
pub async fn process<R: Send + 'static>(
  work: impl FnOnce() -> Result<R, String> + Send + 'static,
) -> Result<R, String> {
  let permit = ADMISSION.clone().acquire_owned().await.map_err(|e| e.to_string())?;
  super::blocking::cpu(move || {
    let _permit = permit;
    work()
  })
  .await
  .map_err(|e| e.to_string())?
}

pub fn prepare(bytes: &[u8]) -> Result<Arc<Prepared>, String> {
  if bytes.len() > MAX_SOURCE_BYTES {
    return Err("image exceeds 100 MiB".into());
  }
  let key: [u8; 32] = Sha256::digest(bytes).into();
  if let Some(result) = cached(&key) {
    return Ok(result);
  }
  // Decoder scratch memory is bounded process-wide, including legacy history projection.
  static DECODER: Mutex<()> = Mutex::new(());
  let _decoder = DECODER.lock().unwrap_or_else(|error| error.into_inner());
  if let Some(result) = cached(&key) {
    return Ok(result);
  }
  let result = Arc::new(convert(bytes)?);
  let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
  cache.retain(|(id, _, _)| id != &key);
  cache.push_back((key, result.clone(), std::time::Instant::now()));
  while cache.len() > 32
    || cache.iter().map(|(_, image, _)| image.bytes.len()).sum::<usize>() > 32 * 1024 * 1024
  {
    cache.pop_front();
  }
  Ok(result)
}
fn cached(key: &[u8; 32]) -> Option<Arc<Prepared>> {
  let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
  let index = cache.iter().position(|(id, _, _)| id == key)?;
  let mut entry = cache.remove(index)?;
  entry.2 = std::time::Instant::now();
  let result = entry.1.clone();
  cache.push_back(entry);
  Some(result)
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
    let dimensions = (picture.width(), picture.height());
    if picture.width().max(picture.height()) <= MAX_EDGE
      && bytes.len() <= MAX_OUTPUT_BYTES
      && orientation == image::metadata::Orientation::NoTransforms
      && matches!(format, Some(image::ImageFormat::Png | image::ImageFormat::Jpeg))
    {
      return Ok(Prepared {
        dimensions,
        bytes: bytes.to_vec(),
        mime: if format == Some(image::ImageFormat::Png) { "image/png" } else { "image/jpeg" },
      });
    }
    if picture.width().max(picture.height()) > MAX_EDGE {
      picture = picture.thumbnail(MAX_EDGE, MAX_EDGE);
    }
    let alpha =
      picture.color().has_alpha() && picture.to_rgba8().pixels().any(|pixel| pixel.0[3] != 255);
    let mut try_png = true;
    loop {
      if try_png {
        let rgba = picture.to_rgba8();
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png).write_image(
          rgba.as_raw(),
          rgba.width(),
          rgba.height(),
          image::ExtendedColorType::Rgba8,
        )?;
        // Preserve text, diagrams and transparency losslessly whenever the budget permits.
        if png.len() <= MAX_OUTPUT_BYTES {
          return Ok(Prepared { dimensions, bytes: png, mime: "image/png" });
        }
        // After an opaque image exceeds the lossless budget, keep JPEG on retries.
        try_png = alpha;
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
            return Ok(Prepared { dimensions, bytes: jpeg, mime: "image/jpeg" });
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
  decode().map_err(|error| format!("Cannot prepare image for model input: {error}. Detected {}. Supported decoders: PNG, JPEG, GIF, WebP, BMP, TIFF; animated files use the first frame. For unsupported formats, try converting a copy to PNG.", super::image_format::unsupported_format(bytes)))
}

/// Admission precedes reading, so concurrent uploads cannot each retain a 100 MiB buffer.
pub async fn prepare_file(path: &std::path::Path) -> Result<Arc<Prepared>, String> {
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

/// Call on a blocking worker: releasing image buffers need not burden the async runtime.
pub fn reap() {
  CACHE
    .lock()
    .unwrap_or_else(|e| e.into_inner())
    .retain(|(_, _, used)| used.elapsed() < CACHE_IDLE);
}
