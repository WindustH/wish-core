use super::{Preview, Source, WORKERS, cached};
use crate::application::{
  app::App,
  attachments,
  error::{ApplicationError, ErrorKind},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
pub async fn prepare(
  app: &Arc<App>,
  owner: &str,
  blob: &str,
  page: Option<u32>,
) -> Result<Source, ApplicationError> {
  let opened = attachments::open(app, owner, blob).await?;
  if opened.mime_type != "application/pdf" {
    return Err(ApplicationError::invalid("PDF preview requires a PDF file"));
  }
  let file = crate::storage::blobs::linked::resolve(&app.data_dir.blobs(owner), blob).await?;
  let metadata = opened.file.metadata().await.map_err(ApplicationError::internal)?;
  let modified = metadata
    .modified()
    .ok()
    .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
    .map(|at| at.as_nanos())
    .unwrap_or(0);
  if page == Some(0) {
    return Err(ApplicationError::invalid("PDF page must be positive"));
  }
  let key = format!(
    "pdf-native-v2:{:?}:{owner}:{blob}:{}:{modified}:{page:?}",
    app.data_dir.root(),
    metadata.len()
  );
  let etag = format!("\"pdf-{:x}\"", Sha256::digest(key.as_bytes()));
  Ok(Source { etag, key, file })
}
pub async fn extract(
  _app: &Arc<App>,
  source: Source,
  page: Option<u32>,
) -> Result<Preview, ApplicationError> {
  let Source { etag, key, file } = source;
  if let Some(bytes) = cached(&key).await {
    return Ok(Preview { bytes });
  }
  let _permit = tokio::time::timeout(std::time::Duration::from_secs(45), WORKERS.acquire())
    .await
    .map_err(|_| ApplicationError {
      kind: ErrorKind::Timeout,
      message: "PDF preview queue timed out".into(),
      details: None,
    })?
    .map_err(ApplicationError::internal)?;
  if let Some(bytes) = cached(&key).await {
    return Ok(Preview { bytes });
  }
  let mut command = super::command(if page.is_some() { "pdf-page" } else { "pdf-info" }, &file)?;
  if let Some(number) = page {
    command.arg(number.to_string());
  }
  let mut result =
    super::execute(command, if page.is_some() { 64 * 1024 * 1024 } else { 2 * 1024 * 1024 })
      .await?;
  if page.is_none() {
    let mut info: serde_json::Value =
      serde_json::from_slice(&result).map_err(ApplicationError::internal)?;
    if info["pages"].as_u64().unwrap_or(0) == 0 {
      return Err(ApplicationError::invalid("PDF has no pages"));
    }
    info["version"] = serde_json::Value::String(etag.clone());
    result = serde_json::to_vec(&info).map_err(ApplicationError::internal)?;
  }
  let bytes = super::remember(key, result).await;
  Ok(Preview { bytes })
}
