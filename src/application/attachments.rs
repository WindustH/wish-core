//! Uploads and reads attachments owned by a session.
use super::{app::App, error::ApplicationError};
use crate::storage::blobs;
pub use crate::storage::blobs::Blob;
use serde::Serialize;
use std::sync::Arc;
/// Requires an existing session: the owners of `blobs/<id>`.
async fn require_owner(app: &Arc<App>, id: &str) -> Result<(), ApplicationError> {
  let management = app.management.clone();
  let data = app.data_dir.clone();
  let storage = app.storage.clone();
  let id = id.to_owned();
  super::error::blocking(move || {
    if !management.exists(&id)? {
      return Err(ApplicationError::not_found());
    }
    super::library::ensure(&data, &storage, &id)?;
    Ok(())
  })
  .await
}
#[derive(Serialize)]
pub struct BlobMetadata {
  id: String,
  mime_type: String,
  byte_count: usize,
  version: String,
  #[serde(flatten)]
  library: blobs::library::Entry,
}

pub async fn upload_stream<S, B, E>(
  app: &Arc<App>,
  owner: &str,
  stream: S,
  temporary: bool,
  name: Option<String>,
) -> Result<Blob, ApplicationError>
where
  S: futures_util::Stream<Item = Result<B, E>>,
  B: AsRef<[u8]>,
  E: std::fmt::Display,
{
  app.lifecycle.require_open()?;
  require_owner(app, owner).await?;
  if name.as_ref().is_some_and(|name| name.len() > 1024) {
    return Err(ApplicationError::invalid("file name exceeds 1024 bytes"));
  }
  let directory = app.data_dir.blobs(owner);
  let blob = blobs::save_upload_stream(&directory, stream, blobs::MAX_UPLOAD_BYTES).await?;
  let id = blob.id.clone();
  super::error::blocking(move || {
    super::env_aliases::Aliases::open(&directory)?.blob(&id)?;
    Ok(blobs::library::record_upload(&directory, &id, name, temporary)?)
  })
  .await?;
  Ok(blob)
}
pub async fn link(
  app: &Arc<App>,
  owner: &str,
  path: &str,
  temporary: bool,
) -> Result<Blob, ApplicationError> {
  app.lifecycle.require_open()?;
  require_owner(app, owner).await?;
  let directory = app.data_dir.blobs(owner);
  let dir = directory.clone();
  let input = path.to_owned();
  let path =
    super::error::blocking(move || super::env_aliases::Aliases::open(&dir)?.expand(&input)).await?;
  let blob = blobs::linked::register(&directory, &path).await?;
  let id = blob.id.clone();
  let name = std::path::Path::new(&blob.path).file_name().map(|s| s.to_string_lossy().into_owned());
  super::error::blocking(move || {
    super::env_aliases::Aliases::open(&directory)?.blob(&id)?;
    Ok(blobs::library::record_upload(&directory, &id, name, temporary)?)
  })
  .await?;
  Ok(blob)
}

pub async fn list(
  app: &Arc<App>,
  owner: &str,
  start: usize,
  limit: usize,
  include_hidden: bool,
) -> Result<blobs::catalog::Page, ApplicationError> {
  list_in(app, owner, start, limit, include_hidden, None).await
}
pub async fn list_in(
  app: &Arc<App>,
  owner: &str,
  start: usize,
  limit: usize,
  include_hidden: bool,
  folder: Option<String>,
) -> Result<blobs::catalog::Page, ApplicationError> {
  require_owner(app, owner).await?;
  super::pagination::validate_limit(limit, 200)?;
  if start > i64::MAX as usize {
    return Err(ApplicationError::invalid("start is too large"));
  }
  let directory = app.data_dir.blobs(owner);
  super::error::blocking(move || {
    let page = if let Some(folder) = folder {
      blobs::catalog::list_in(&directory, start, limit, include_hidden, Some(&folder))?
    } else {
      blobs::catalog::list(&directory, start, limit, include_hidden)?
    };
    let aliases = super::env_aliases::Aliases::open(&directory)?;
    aliases.register(&page.items.iter().map(|item| item.id.clone()).collect::<Vec<_>>(), &[])?;
    Ok(page)
  })
  .await
}
pub async fn create_folder(
  app: &Arc<App>,
  owner: &str,
  input: blobs::library::FolderInput,
) -> Result<blobs::library::Folder, ApplicationError> {
  app.lifecycle.require_open()?;
  require_owner(app, owner).await?;
  let directory = app.data_dir.blobs(owner);
  super::error::blocking(move || Ok(blobs::library::create_folder(&directory, input)?)).await
}
pub async fn rename_folder(
  app: &Arc<App>,
  owner: &str,
  id: String,
  name: String,
) -> Result<blobs::library::Folder, ApplicationError> {
  app.lifecycle.require_open()?;
  require_owner(app, owner).await?;
  let directory = app.data_dir.blobs(owner);
  super::error::blocking(move || Ok(blobs::library::rename_folder(&directory, &id, &name)?)).await
}
pub async fn delete_folder(
  app: &Arc<App>,
  owner: &str,
  id: String,
) -> Result<(), ApplicationError> {
  app.lifecycle.require_open()?;
  require_owner(app, owner).await?;
  let directory = app.data_dir.blobs(owner);
  super::error::blocking(move || Ok(blobs::library::delete_folder(&directory, &id)?)).await
}
pub async fn move_library_items(
  app: &Arc<App>,
  owner: &str,
  input: blobs::library::FolderMove,
) -> Result<(), ApplicationError> {
  app.lifecycle.require_open()?;
  require_owner(app, owner).await?;
  let directory = app.data_dir.blobs(owner);
  super::error::blocking(move || Ok(blobs::library::move_items(&directory, input)?)).await
}
pub async fn update_library(
  app: &Arc<App>,
  owner: &str,
  blob: &str,
  patch: blobs::library::Patch,
) -> Result<blobs::library::Entry, ApplicationError> {
  app.lifecycle.require_open()?;
  require_owner(app, owner).await?;
  let directory = app.data_dir.blobs(owner);
  let blob = blob.to_owned();
  super::error::blocking(move || {
    if !blobs::catalog::contains(&directory, &blob)? {
      return Err(ApplicationError::not_found());
    }
    Ok(blobs::library::update(&directory, &blob, patch)?)
  })
  .await
}
pub async fn delete(app: &Arc<App>, owner: &str, blob: &str) -> Result<(), ApplicationError> {
  app.lifecycle.require_open()?;
  require_owner(app, owner).await?;
  if !blobs::is_blob_id(blob) {
    return Err(ApplicationError::not_found());
  }
  let directory = app.data_dir.blobs(owner);
  let blob = blob.to_owned();
  let slot = app.get_session(owner).await?;
  super::error::blocking(move || {
    slot.require_live()?;
    slot.sender.with_image_dependencies(move |images| {
      if images.contains(&blob) {
        return Err(ApplicationError::conflict("This image is required by the current or standby generation, or a pending message; hide it from the Library instead"));
      }
      Ok(blobs::library::delete(&directory, &blob)?)
    })?
  }).await
}
pub struct BlobReader {
  pub file: tokio::fs::File,
  pub length: u64,
  pub mime_type: String,
  pub version: String,
}
pub async fn open(app: &Arc<App>, owner: &str, blob: &str) -> Result<BlobReader, ApplicationError> {
  use tokio::io::{AsyncReadExt, AsyncSeekExt};
  require_owner(app, owner).await?;
  if !blobs::is_blob_id(blob) {
    return Err(ApplicationError::not_found());
  }
  let mut file =
    tokio::fs::File::open(blobs::linked::resolve(&app.data_dir.blobs(owner), blob).await?)
      .await
      .map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
          ApplicationError::not_found()
        } else {
          ApplicationError::internal(error)
        }
      })?;
  let metadata = file.metadata().await.map_err(ApplicationError::internal)?;
  let length = metadata.len();
  let modified = metadata
    .modified()
    .ok()
    .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
    .map(|at| at.as_nanos())
    .unwrap_or(0);
  let version = format!("\"{blob}-{length}-{modified}\"");
  let mut prefix = [0; 512];
  let count = file.read(&mut prefix).await.map_err(ApplicationError::internal)?;
  file.rewind().await.map_err(ApplicationError::internal)?;
  let mime_type = blobs::sniff_mime_type(&prefix[..count]).into();
  Ok(BlobReader { file, length, mime_type, version })
}
pub async fn metadata(
  app: &Arc<App>,
  owner: &str,
  blob: &str,
) -> Result<BlobMetadata, ApplicationError> {
  let item = open(app, owner, blob).await?;
  let directory = app.data_dir.blobs(owner);
  let id = blob.to_owned();
  let library = super::error::blocking(move || {
    Ok(blobs::library::entries(&directory)?.remove(&id).unwrap_or_default())
  })
  .await?;
  Ok(BlobMetadata {
    id: blob.into(),
    mime_type: item.mime_type,
    byte_count: usize::try_from(item.length).map_err(ApplicationError::internal)?,
    version: item.version,
    library,
  })
}

/// Stage bytes under a submission UUID without creating or publishing an empty session.
/// Content-addressed names make retries reuse the same file; acceptance adopts this directory.
pub async fn upload_draft_stream<S, B, E>(
  app: &Arc<App>,
  request_id: &str,
  stream: S,
) -> Result<Blob, ApplicationError>
where
  S: futures_util::Stream<Item = Result<B, E>>,
  B: AsRef<[u8]>,
  E: std::fmt::Display,
{
  app.lifecycle.require_open()?;
  let id = uuid::Uuid::parse_str(request_id)
    .map_err(|_| ApplicationError::invalid("draft request_id must be a UUID"))?
    .to_string();
  super::draft_uploads::upload_stream(app, id, stream).await
}
