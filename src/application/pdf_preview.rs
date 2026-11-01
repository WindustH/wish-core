//! Bounded server-side PDF metadata and page rendering. Originals never leave the
//! backend for a preview; MuPDF reads the local file and only its output is cached.
use super::{
  app::App,
  attachments,
  error::{ApplicationError, ErrorKind},
};
use sha2::{Digest, Sha256};
use std::{
  collections::VecDeque,
  process::Stdio,
  sync::{Arc, LazyLock},
};
use tokio::{
  io::AsyncReadExt,
  sync::{Mutex, Semaphore},
};

const LIMIT: usize = 128 * 1024 * 1024;
type Entries = VecDeque<(String, Arc<Vec<u8>>)>;
static CACHE: LazyLock<Mutex<Entries>> = LazyLock::new(|| Mutex::new(VecDeque::new()));
static WORKERS: Semaphore = Semaphore::const_new(2);
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

async fn cached(key: &str) -> Option<Arc<Vec<u8>>> {
  let mut cache = CACHE.lock().await;
  let position = cache.iter().position(|(id, _)| id == key)?;
  let entry = cache.remove(position)?;
  let data = entry.1.clone();
  cache.push_back(entry);
  Some(data)
}

pub struct Preview {
  pub bytes: Arc<Vec<u8>>,
  pub etag: String,
}
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
  command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
  let mut child = command.spawn().map_err(|error| {
    if error.kind() == std::io::ErrorKind::NotFound {
      ApplicationError {
        kind: ErrorKind::Unsupported,
        message: "PDF previews require mutool (mupdf-tools) on the Wish server".into(),
        details: None,
      }
    } else {
      ApplicationError::internal(error)
    }
  })?;
  let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
    let mut bytes = Vec::new();
    let limit = if page.is_some() { 16 * 1024 * 1024 } else { 2 * 1024 * 1024 };
    child
      .stdout
      .take()
      .expect("piped output")
      .take(limit + 1)
      .read_to_end(&mut bytes)
      .await
      .map_err(ApplicationError::internal)?;
    if bytes.len() as u64 > limit {
      return Err(ApplicationError::invalid("PDF preview output exceeds limit"));
    }
    let status = child.wait().await.map_err(ApplicationError::internal)?;
    if !status.success() || bytes.is_empty() {
      return Err(ApplicationError::invalid(
        "Unable to preview this PDF or page (encrypted, damaged, or invalid page)",
      ));
    }
    if page.is_none() {
      let info: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(ApplicationError::internal)?;
      if info["pages"].as_u64().unwrap_or(0) == 0 {
        return Err(ApplicationError::invalid("PDF has no pages"));
      }
    }
    Ok(bytes)
  })
  .await
  .map_err(|_| ApplicationError {
    kind: ErrorKind::Timeout,
    message: "PDF preview timed out".into(),
    details: None,
  })??;
  let bytes = Arc::new(result);
  let mut cache = CACHE.lock().await;
  cache.push_back((key, bytes.clone()));
  while cache.len() > 128 || cache.iter().map(|(_, bytes)| bytes.len()).sum::<usize>() > LIMIT {
    cache.pop_front();
  }
  Ok(Preview { bytes, etag })
}
