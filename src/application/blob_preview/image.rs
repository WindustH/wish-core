use super::{Preview, WORKERS, cached};
use crate::application::{
  app::App,
  attachments,
  error::{ApplicationError, ErrorKind},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub async fn read(
  app: &Arc<App>,
  owner: &str,
  blob: &str,
  width: u32,
) -> Result<Preview, ApplicationError> {
  if ![384, 768, 1536].contains(&width) {
    return Err(ApplicationError::invalid("Image preview width must be 384, 768 or 1536"));
  }
  let opened = attachments::open(app, owner, blob).await?;
  if !matches!(opened.mime_type.as_str(), "image/png" | "image/jpeg" | "image/webp" | "image/gif") {
    return Err(ApplicationError::invalid("Image preview requires PNG, JPEG, WebP or GIF"));
  }
  let file = crate::storage::blobs::linked::resolve(&app.data_dir.blobs(owner), blob).await?;
  let metadata = opened.file.metadata().await.map_err(ApplicationError::internal)?;
  let modified = metadata
    .modified()
    .ok()
    .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
    .map(|at| at.as_nanos())
    .unwrap_or(0);
  let key = format!(
    "image-v1:{:?}:{owner}:{blob}:{}:{modified}:{width}",
    app.data_dir.root(),
    metadata.len()
  );
  let etag = format!("\"image-{:x}\"", Sha256::digest(key.as_bytes()));
  if let Some(bytes) = cached(&key).await {
    return Ok(Preview { bytes, etag });
  }
  let _permit = tokio::time::timeout(std::time::Duration::from_secs(45), WORKERS.acquire())
    .await
    .map_err(|_| ApplicationError {
      kind: ErrorKind::Timeout,
      message: "Image preview queue timed out".into(),
      details: None,
    })?
    .map_err(ApplicationError::internal)?;
  if let Some(bytes) = cached(&key).await {
    return Ok(Preview { bytes, etag });
  }
  let mut command = tokio::process::Command::new("magick");
  command
    .args([
      "-limit", "memory", "128MiB", "-limit", "map", "128MiB", "-limit", "disk", "0", "-limit",
      "thread", "1",
    ])
    .arg(format!("{}[0]", file.display()))
    .args([
      "-auto-orient",
      "-thumbnail",
      &format!("{width}x{width}>"),
      "-strip",
      "-quality",
      "80",
      "webp:-",
    ]);
  let result = super::execute(command, "magick (ImageMagick)", 16 * 1024 * 1024).await?;
  let bytes = super::remember(key, result).await;
  Ok(Preview { bytes, etag })
}
