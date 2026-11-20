//! Read only the media job fields required to protect blobs during reclamation.
use serde::Deserialize;
use std::{
  io,
  path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct BlobRef {
  id: String,
}

#[derive(Deserialize)]
pub(crate) struct JobResources {
  pub id: Option<String>,
  phase: String,
  artifacts: Vec<BlobRef>,
  timestamps: Option<BlobRef>,
}

impl JobResources {
  pub fn terminal(&self) -> bool {
    matches!(self.phase.as_str(), "succeeded" | "failed" | "outcome_unknown")
  }
  pub fn blob_ids(self) -> impl Iterator<Item = String> {
    self.artifacts.into_iter().chain(self.timestamps).map(|blob| blob.id)
  }
}

/// An unreadable record leaves its dependencies unknown. Both cleanup paths retain candidates.
pub(crate) fn visit(
  directory: &Path,
  mut visit: impl FnMut(PathBuf, JobResources),
) -> io::Result<u64> {
  let entries = match std::fs::read_dir(directory.join(".media-jobs")) {
    Ok(entries) => entries,
    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
    Err(error) => return Err(error),
  };
  let mut unreadable = 0;
  for entry in entries {
    let path = entry?.path();
    if path.extension().is_none_or(|ext| ext != "json") {
      continue;
    }
    match crate::utils::bounded_file::read_sync(&path, 2 * 1024 * 1024)
      .ok()
      .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
      Some(job) => visit(path, job),
      None => unreadable += 1,
    }
  }
  Ok(unreadable)
}
