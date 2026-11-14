//! Pure file signatures shared by artifact validation and storage.
pub fn mime(bytes: &[u8]) -> Option<&'static str> {
  if bytes.starts_with(b"%PDF-") {
    return Some("application/pdf");
  }
  if bytes.starts_with(b"PK\x03\x04") {
    return Some("application/zip");
  }
  if bytes.starts_with(b"ID3") || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
  {
    return Some("audio/mpeg");
  }
  if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
    return Some("audio/wav");
  }
  if bytes.starts_with(b"fLaC") {
    return Some("audio/flac");
  }
  if bytes.starts_with(b"OggS") {
    return Some("audio/ogg");
  }
  if bytes.get(4..8) == Some(b"ftyp") {
    return Some(if bytes.get(8..12) == Some(b"qt  ") { "video/quicktime" } else { "video/mp4" });
  }
  if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
    return match ebml_document_type(bytes) {
      Some(b"webm") => Some("video/webm"),
      Some(b"matroska") => Some("video/x-matroska"),
      _ => None,
    };
  }
  None
}

// EBML containers share a magic number; the header's DocType distinguishes them.
fn ebml_document_type(bytes: &[u8]) -> Option<&[u8]> {
  fn vint(bytes: &[u8], keep_marker: bool) -> Option<(usize, usize)> {
    let first = *bytes.first()?;
    let length = first.leading_zeros() as usize + 1;
    if length > 8 || bytes.len() < length {
      return None;
    }
    let mut value =
      usize::from(if keep_marker { first } else { first & (0xff_u16 >> length) as u8 });
    for byte in &bytes[1..length] {
      value = value.checked_mul(256)?.checked_add(usize::from(*byte))?;
    }
    Some((value, length))
  }
  let (size, length) = vint(bytes.get(4..)?, false)?;
  let mut offset = 4 + length;
  let header = bytes.get(..offset.checked_add(size)?.min(bytes.len()))?;
  while offset < header.len() {
    let (id, length) = vint(&header[offset..], true)?;
    offset += length;
    let (size, length) = vint(&header[offset..], false)?;
    offset += length;
    let end = offset.checked_add(size)?;
    let value = header.get(offset..end)?;
    if id == 0x4282 {
      return Some(value);
    }
    offset = end;
  }
  None
}
