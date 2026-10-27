//! Image signatures shared by uploads and local image tools; extensions are never authoritative.
pub fn supported_mime(bytes: &[u8]) -> Option<&'static str> {
  if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
    Some("image/png")
  } else if bytes.starts_with(b"\xff\xd8\xff") {
    Some("image/jpeg")
  } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
    Some("image/gif")
  } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
    Some("image/webp")
  } else {
    None
  }
}

pub fn unsupported_format(bytes: &[u8]) -> &'static str {
  if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
    let brands = &bytes[8..bytes.len().min(40)];
    if brands.windows(4).any(|brand| matches!(brand, b"avif" | b"avis")) {
      return "AVIF";
    }
    if brands
      .windows(4)
      .any(|brand| matches!(brand, b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1"))
    {
      return "HEIF/HEIC";
    }
  }
  if bytes.starts_with(b"BM") {
    return "BMP";
  }
  if bytes.starts_with(b"II\x2a\x00") || bytes.starts_with(b"MM\x00\x2a") {
    return "TIFF";
  }
  "unknown or corrupt data"
}

pub fn conversion_hint(bytes: &[u8]) -> String {
  format!(
    "Cannot view image: detected {} from file content. Supported: PNG, JPEG, GIF, WebP. Try converting to a new PNG using an available decoder (for example ImageMagick or ffmpeg), then call view_image with its absolute path. Preserve the original; renaming the extension is not conversion.",
    unsupported_format(bytes)
  )
}
