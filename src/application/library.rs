//! One-time recovery of the user's existing attachments. Only metadata is read, never full
//! message bodies or file contents. Unclassified artifacts remain accessible in All files.
use crate::storage::{
  blobs::{is_blob_id, library::Entry},
  data_dir::DataDir,
  repository_error::RepositoryError,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

pub(crate) fn attachments(metadata: &Value) -> BTreeMap<String, Entry> {
  let crops: HashSet<&str> = metadata["custom"]["reply_references"]
    .as_array()
    .into_iter()
    .flatten()
    .filter(|reference| reference["region"].is_object())
    .filter_map(|reference| reference["imageId"].as_str())
    .collect();
  metadata["attachments"]
    .as_array()
    .into_iter()
    .flatten()
    .filter_map(|attachment| {
      let id = attachment["id"].as_str().filter(|id| is_blob_id(id) && !crops.contains(id))?;
      Some((
        id.into(),
        Entry {
          visible: true,
          pinned: false,
          name: attachment["name"].as_str().map(String::from),
          ..Default::default()
        },
      ))
    })
    .collect()
}

pub(super) fn recover(
  data: &DataDir,
  storage: &crate::storage::Storage,
  owner: &str,
) -> Result<BTreeMap<String, Entry>, RepositoryError> {
  let directory = data.blobs(owner);
  let mut entries = BTreeMap::new();
  // Local links have always been explicitly registered by the user, including links whose
  // source has since disappeared. Their descriptors are sufficient to restore membership.
  match std::fs::read_dir(&directory) {
    Ok(files) => {
      for file in files {
        let file = file.map_err(RepositoryError::internal)?;
        let name = file.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_suffix(".json").filter(|id| is_blob_id(id)) else { continue };
        let bytes = crate::utils::bounded_file::read_sync(&file.path(), 64 * 1024)
          .map_err(RepositoryError::internal)?;
        if let Ok(blob) = serde_json::from_slice::<crate::storage::blobs::Blob>(&bytes)
          && blob.linked
          && blob.id == id
        {
          entries.insert(
            id.into(),
            Entry { visible: true, pinned: false, name: None, ..Default::default() },
          );
        }
      }
    }
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
    Err(error) => return Err(RepositoryError::internal(error)),
  }
  for metadata in crate::session::SessionReader::attachment_metadata(storage, owner)
    .map_err(RepositoryError::internal)?
  {
    entries.extend(attachments(&metadata));
  }
  Ok(entries)
}

/// Call in admitted blocking work before a Library operation or existing-session input.
pub(crate) fn ensure(
  data: &DataDir,
  storage: &crate::storage::Storage,
  owner: &str,
) -> Result<(), RepositoryError> {
  let directory = data.blobs(owner);
  crate::storage::blobs::library::initialize(&directory, || recover(data, storage, owner))
}
