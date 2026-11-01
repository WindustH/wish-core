use super::{Preview, WORKERS, cached};
use crate::application::{
  app::App,
  attachments,
  error::{ApplicationError, ErrorKind},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;
const SCRIPT: &str = r#"
var d = Document.openDocument(scriptArgs[0]);
var rows = [];
function walk(items, depth) {
  if (!items || depth > 32) return;
  for (var i=0; i<items.length && rows.length<10000; i++) {
    var item = items[i], page = null;
    try { if (item.uri && item.uri.charAt(0)==='#') page=d.resolveLink(item.uri)+1; } catch(e) {}
    rows.push({title:item.title || '', depth:depth, page:page});
    walk(item.down, depth+1);
  }
}
try { walk(d.loadOutline(), 0); } catch(e) {}
print(JSON.stringify({pages:d.countPages(),bookmarks:rows}));
"#;

pub async fn read(
  app: &Arc<App>,
  owner: &str,
  blob: &str,
  page: Option<(u32, u32)>,
) -> Result<Preview, ApplicationError> {
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
  if let Some((number, width)) = page
    && (number == 0 || !(128..=2048).contains(&width))
  {
    return Err(ApplicationError::invalid("PDF page must be positive and width must be 128..2048"));
  }
  let key =
    format!("{:?}:{owner}:{blob}:{}:{modified}:{page:?}", app.data_dir.root(), metadata.len());
  let etag = format!("\"pdf-{:x}\"", Sha256::digest(key.as_bytes()));
  if let Some(bytes) = cached(&key).await {
    return Ok(Preview { bytes, etag });
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
    return Ok(Preview { bytes, etag });
  }
  let mut command = tokio::process::Command::new("mutool");
  if let Some((number, width)) = page {
    command
      .args([
        "draw",
        "-q",
        "-F",
        "png",
        "-o",
        "-",
        "-w",
        &width.to_string(),
        "-h",
        &width.to_string(),
      ])
      .arg(&file)
      .arg(number.to_string());
  } else {
    let directory = app.data_dir.root().join("preview-tools");
    tokio::fs::create_dir_all(&directory).await.map_err(ApplicationError::internal)?;
    let script = directory.join("pdf-info-v1.js");
    // Immutable contents; atomic rename avoids concurrent readers seeing a partial script.
    if !script.exists() {
      let temporary = directory.join(format!("{}.tmp", uuid::Uuid::new_v4()));
      tokio::fs::write(&temporary, SCRIPT).await.map_err(ApplicationError::internal)?;
      tokio::fs::rename(&temporary, &script).await.map_err(ApplicationError::internal)?;
    }
    command.arg("run").arg(script).arg(&file);
  }
  let result = super::execute(
    command,
    "mutool (mupdf-tools)",
    if page.is_some() { 16 * 1024 * 1024 } else { 2 * 1024 * 1024 },
  )
  .await?;
  if page.is_none() {
    let info: serde_json::Value =
      serde_json::from_slice(&result).map_err(ApplicationError::internal)?;
    if info["pages"].as_u64().unwrap_or(0) == 0 {
      return Err(ApplicationError::invalid("PDF has no pages"));
    }
  }
  let bytes = super::remember(key, result).await;
  Ok(Preview { bytes, etag })
}
