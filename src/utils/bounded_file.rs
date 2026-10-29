//! Bounded reads for local records, including files changed after metadata inspection.
use std::{io, path::Path};
fn too_large() -> io::Error {
  io::Error::new(io::ErrorKind::InvalidData, "record exceeds size limit")
}
pub async fn read(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
  use tokio::io::AsyncReadExt;
  let file = tokio::fs::File::open(path).await?;
  if file.metadata().await?.len() > limit as u64 {
    return Err(too_large());
  }
  let mut bytes = Vec::new();
  file.take(limit as u64 + 1).read_to_end(&mut bytes).await?;
  if bytes.len() > limit {
    return Err(too_large());
  }
  Ok(bytes)
}
pub fn read_sync(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
  use std::io::Read;
  let file = std::fs::File::open(path)?;
  if file.metadata()?.len() > limit as u64 {
    return Err(too_large());
  }
  let mut bytes = Vec::new();
  file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
  if bytes.len() > limit {
    return Err(too_large());
  }
  Ok(bytes)
}
