//! Linked directories and bookmarks coordinate owner lifetimes with Library persistence.
use super::{
  app::App,
  attachments,
  error::{ApplicationError, blocking},
  session::SessionSlot,
};
use crate::{
  session::history::query::HistoryContent,
  storage::blobs::{
    self,
    library::{
      self,
      bookmarks::{self, Bookmark},
      linked_folders,
    },
  },
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
mod bookmark_text;

async fn owner(app: &Arc<App>, id: &str) -> Result<Arc<SessionSlot>, ApplicationError> {
  app.lifecycle.require_open()?;
  attachments::require_owner(app, id).await?;
  app.get_session(id).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkPath {
  pub path: String,
  pub parent: Option<String>,
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinkedItem {
  File { blob: blobs::Blob },
  Folder { folder: library::Folder },
}
/// One user-facing link operation; file-only tool/CLI contracts remain unchanged.
pub async fn link_path(
  app: &Arc<App>,
  id: &str,
  input: LinkPath,
) -> Result<LinkedItem, ApplicationError> {
  if input.path.is_empty() || input.path.len() > 4096 {
    return Err(ApplicationError::invalid("path must be 1–4096 bytes"));
  }
  let slot = owner(app, id).await?;
  let publication = slot.blob_publication.clone().read_owned().await;
  slot.require_live()?;
  let resolve_slot = slot.clone();
  let parent = input.parent;
  let destination = parent.clone();
  let path = blocking(move || {
    resolve_slot.require_live()?;
    library::check_parent(&resolve_slot.blob_dir, destination.as_deref())?;
    resolve_slot.aliases.expand(&input.path)
  })
  .await?;
  if !std::path::Path::new(&path).is_absolute() {
    return Err(ApplicationError::invalid("local path must be absolute"));
  }
  let metadata = tokio::fs::metadata(&path).await.map_err(|error| {
    if error.kind() == std::io::ErrorKind::NotFound {
      ApplicationError::not_found()
    } else {
      ApplicationError::invalid(format!("cannot access path: {error}"))
    }
  })?;
  if metadata.is_dir() {
    return blocking(move || {
      let _publication = publication;
      slot.require_live()?;
      let folder =
        linked_folders::create(&slot.blob_dir, linked_folders::Link { path, name: None, parent })?;
      Ok(LinkedItem::Folder { folder })
    })
    .await;
  }
  if !metadata.is_file() {
    return Err(ApplicationError::invalid("path must be a regular file or directory"));
  }
  let blob = blobs::linked::register(&slot.blob_dir, &path).await?;
  blocking(move || {
    let _publication = publication;
    slot.require_live()?;
    slot.aliases.blob(&blob.id)?;
    let name =
      std::path::Path::new(&blob.path).file_name().map(|name| name.to_string_lossy().into_owned());
    library::record_link(&slot.blob_dir, &blob.id, name, parent)?;
    Ok(LinkedItem::File { blob })
  })
  .await
}
pub async fn link_folder(
  app: &Arc<App>,
  id: &str,
  mut input: linked_folders::Link,
) -> Result<library::Folder, ApplicationError> {
  let slot = owner(app, id).await?;
  let publication = slot.blob_publication.clone().read_owned().await;
  blocking(move || {
    let _publication = publication;
    slot.require_live()?;
    input.path = slot.aliases.expand(&input.path)?;
    Ok(linked_folders::create(&slot.blob_dir, input)?)
  })
  .await
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Browse {
  pub path: String,
  pub after: Option<String>,
  pub limit: usize,
}
impl Default for Browse {
  fn default() -> Self {
    Self { path: String::new(), after: None, limit: 80 }
  }
}
pub async fn browse(
  app: &Arc<App>,
  id: &str,
  folder: String,
  query: Browse,
) -> Result<linked_folders::Page, ApplicationError> {
  let slot = owner(app, id).await?;
  blocking(move || {
    slot.require_live()?;
    Ok(linked_folders::list(
      &slot.blob_dir,
      &folder,
      &query.path,
      query.after.as_deref(),
      query.limit,
    )?)
  })
  .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildName {
  pub path: String,
  pub name: String,
}
pub async fn rename_child(
  app: &Arc<App>,
  id: &str,
  folder: String,
  input: ChildName,
) -> Result<(), ApplicationError> {
  let slot = owner(app, id).await?;
  let publication = slot.blob_publication.clone().read_owned().await;
  blocking(move || {
    let _publication = publication;
    slot.require_live()?;
    Ok(linked_folders::rename(&slot.blob_dir, &folder, &input.path, &input.name)?)
  })
  .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildPath {
  pub path: String,
}
pub async fn open_child(
  app: &Arc<App>,
  id: &str,
  folder: String,
  input: ChildPath,
) -> Result<blobs::Blob, ApplicationError> {
  let slot = owner(app, id).await?;
  let publication = slot.blob_publication.clone().read_owned().await;
  slot.require_live()?;
  let directory = slot.blob_dir.clone();
  let (path, display_name) =
    blocking(move || Ok(linked_folders::file(&directory, &folder, &input.path)?)).await?;
  let blob = blobs::linked::register(&slot.blob_dir, &path).await?;
  let blob_id = blob.id.clone();
  blocking(move || {
    let _publication = publication;
    slot.require_live()?;
    slot.aliases.blob(&blob_id)?;
    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned());
    library::record_link_preview(&slot.blob_dir, &blob_id, name, display_name)?;
    Ok(())
  })
  .await?;
  Ok(blob)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookmarkInput {
  pub seq: u64,
  pub block: Option<usize>,
  pub start: Option<usize>,
  pub end: Option<usize>,
}
pub async fn add_bookmark(
  app: &Arc<App>,
  id: &str,
  input: BookmarkInput,
) -> Result<Bookmark, ApplicationError> {
  let slot = owner(app, id).await?;
  let publication = slot.blob_publication.clone().read_owned().await;
  blocking(move || {
    let _publication = publication;
    slot.require_live()?;
    let item = slot.reader.read_history_item(input.seq)?.ok_or_else(ApplicationError::not_found)?;
    let HistoryContent::Message(entry) = item.content else {
      return Err(ApplicationError::invalid("bookmark must target a message"));
    };
    let content = bookmark_text::blocks(&entry.message).ok_or_else(|| {
      ApplicationError::invalid("bookmark must target a user or assistant message")
    })?;
    let text = match (input.block, input.start, input.end) {
      (Some(block), Some(start), Some(end)) if start < end && end - start <= 16000 => {
        let Some(Some(text)) = content.get(block) else {
          return Err(ApplicationError::invalid("invalid bookmark text block"));
        };
        // Browser ranges are UTF-16 offsets; reject surrogate splits instead of relocating text.
        let units: Vec<_> = text.encode_utf16().skip(start).take(end - start).collect();
        if units.len() != end - start {
          return Err(ApplicationError::invalid("bookmark range is out of bounds"));
        }
        String::from_utf16(&units)
          .map_err(|_| ApplicationError::invalid("bookmark range splits a character"))?
      }
      (None, None, None) => content
        .iter()
        .filter_map(|block| block.as_deref())
        .flat_map(|text| text.chars().chain(['\n']))
        .take(4000)
        .collect(),
      _ => return Err(ApplicationError::invalid("supply block, start and end together")),
    };
    let name: String =
      text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(80).collect();
    Ok(bookmarks::add(
      &slot.blob_dir,
      Bookmark {
        id: format!("bookmark_{}", uuid::Uuid::new_v4()),
        seq: input.seq,
        block: input.block,
        start: input.start,
        end: input.end,
        text,
        name: if name.is_empty() { "Message".into() } else { name },
        created_at: crate::utils::time::Timestamp::now().0,
      },
    )?)
  })
  .await
}
pub async fn list_bookmarks(app: &Arc<App>, id: &str) -> Result<Vec<Bookmark>, ApplicationError> {
  let slot = owner(app, id).await?;
  blocking(move || {
    slot.require_live()?;
    Ok(bookmarks::list(&slot.blob_dir)?)
  })
  .await
}
pub async fn edit_bookmark(
  app: &Arc<App>,
  id: &str,
  bookmark: String,
  name: Option<String>,
) -> Result<(), ApplicationError> {
  let slot = owner(app, id).await?;
  let publication = slot.blob_publication.clone().read_owned().await;
  blocking(move || {
    let _publication = publication;
    slot.require_live()?;
    if let Some(name) = name {
      bookmarks::rename(&slot.blob_dir, &bookmark, &name)?;
    } else {
      bookmarks::delete(&slot.blob_dir, &bookmark)?;
    }
    Ok(())
  })
  .await
}
