//! Bounded validation of PNG sequence archives; never extracts files or decodes image pixels.
use std::{
  fs::File,
  io::{Read, Seek, SeekFrom},
  path::Path,
};
const MAX_ENTRIES: usize = 10_000;
const MAX_DIRECTORY: usize = 8 * 1024 * 1024;
const MAX_EXPANDED: u64 = 2 * 1024 * 1024 * 1024;
fn u16_at(bytes: &[u8], offset: usize) -> u16 {
  u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
  u32::from_le_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]])
}
fn io(error: impl std::fmt::Display) -> String {
  format!("invalid PNG sequence ZIP: {error}")
}
pub(crate) fn validate(path: &Path) -> Result<(), String> {
  let mut file = File::open(path).map_err(io)?;
  let length = file.metadata().map_err(io)?.len();
  let tail_size = length.min(65_557) as usize;
  let mut tail = vec![0; tail_size];
  file.seek(SeekFrom::End(-(tail_size as i64))).map_err(io)?;
  file.read_exact(&mut tail).map_err(io)?;
  let end = (0..tail.len().saturating_sub(21))
    .rev()
    .find(|&offset| {
      tail.get(offset..offset + 4) == Some(b"PK\x05\x06")
        && offset + 22 + usize::from(u16_at(&tail, offset + 20)) == tail.len()
    })
    .ok_or_else(|| io("missing end-of-directory record"))?;
  let entries = usize::from(u16_at(&tail, end + 10));
  let size = u32_at(&tail, end + 12) as usize;
  let offset = u64::from(u32_at(&tail, end + 16));
  if u16_at(&tail, end + 4) != 0
    || u16_at(&tail, end + 6) != 0
    || usize::from(u16_at(&tail, end + 8)) != entries
    || entries == 0
    || entries > MAX_ENTRIES
    || size > MAX_DIRECTORY
    || offset + size as u64 != length - tail_size as u64 + end as u64
  {
    return Err(io("unsupported directory layout or archive limits exceeded"));
  }
  // Check lengths/counts before the library allocates its metadata. This bounded format excludes
  // multi-disk/ZIP64 central directories (unneeded below the advertised 512 MiB / 10,000-entry limits).
  let mut directory = vec![0; size];
  file.seek(SeekFrom::Start(offset)).map_err(io)?;
  file.read_exact(&mut directory).map_err(io)?;
  let mut cursor = 0;
  let mut expanded = 0u64;
  for _ in 0..entries {
    if cursor + 46 > size || directory.get(cursor..cursor + 4) != Some(b"PK\x01\x02") {
      return Err(io("invalid central directory"));
    }
    expanded = expanded
      .checked_add(u64::from(u32_at(&directory, cursor + 24)))
      .ok_or_else(|| io("expanded size overflow"))?;
    cursor += 46
      + usize::from(u16_at(&directory, cursor + 28))
      + usize::from(u16_at(&directory, cursor + 30))
      + usize::from(u16_at(&directory, cursor + 32));
    if cursor > size || expanded > MAX_EXPANDED {
      return Err(io("expanded archive limit exceeded"));
    }
  }
  if cursor != size {
    return Err(io("unexpected directory data"));
  }
  file.seek(SeekFrom::Start(0)).map_err(io)?;
  let mut archive = zip::ZipArchive::new(file).map_err(io)?;
  if archive.len() != entries {
    return Err(io("duplicate or inconsistent directory entries"));
  }
  archive.validate_header_consistency().map_err(io)?;
  let mut frames = 0;
  let mut total = 0u64;
  let mut names = std::collections::HashSet::new();
  for index in 0..archive.len() {
    let mut entry = archive.by_index(index).map_err(io)?;
    let name = entry.name().map_err(io)?.into_owned();
    if entry.enclosed_name().is_none()
      || name.contains(['\\', ':'])
      || name.chars().any(char::is_control)
      || name.split('/').any(|part| part == "..")
      || !names.insert(name.clone())
      || entry.encrypted()
      || entry.is_symlink()
    {
      return Err(io("unsafe, duplicate or encrypted entry"));
    }
    if entry.is_dir() {
      continue;
    }
    if !name.to_ascii_lowercase().ends_with(".png")
      || entry.size() < 8
      || entry.size() > 256 * 1024 * 1024
    {
      return Err(io("expected bounded PNG frames"));
    }
    let mut signature = [0; 8];
    entry.read_exact(&mut signature).map_err(io)?;
    if signature != *b"\x89PNG\r\n\x1a\n" {
      return Err(io("frame is not PNG"));
    }
    let limit = entry.size();
    let count = std::io::copy(&mut entry.take(limit + 1), &mut std::io::sink()).map_err(io)?;
    if count + 8 != limit {
      return Err(io("frame size mismatch"));
    }
    total = total
      .checked_add(count + 8)
      .filter(|n| *n <= MAX_EXPANDED)
      .ok_or_else(|| io("expanded limit exceeded"))?;
    frames += 1;
  }
  if frames == 0 {
    return Err(io("archive has no PNG frames"));
  }
  Ok(())
}
