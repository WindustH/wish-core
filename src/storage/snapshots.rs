//! Immutable JSON snapshots addressed by opaque UUIDs.
use super::repository_error::RepositoryError;
use serde::{Serialize, de::DeserializeOwned};
use std::path::PathBuf;
#[derive(Clone)]
pub struct SnapshotStore {
  directory: PathBuf,
}
impl SnapshotStore {
  pub fn new(directory: PathBuf) -> Self {
    Self { directory }
  }
  pub async fn read<T: DeserializeOwned>(&self, id: uuid::Uuid) -> Result<T, RepositoryError> {
    let bytes =
      crate::utils::bounded_file::read(&self.directory.join(format!("{id}.json")), 8 * 1024 * 1024)
        .await
        .map_err(RepositoryError::internal)?;
    serde_json::from_slice(&bytes).map_err(RepositoryError::internal)
  }
  pub async fn insert<T: Serialize>(&self, value: &T) -> Result<uuid::Uuid, RepositoryError> {
    let id = uuid::Uuid::new_v4();
    let bytes = serde_json::to_vec(value).map_err(RepositoryError::internal)?;
    if bytes.len() > 8 * 1024 * 1024 {
      return Err(RepositoryError::internal("snapshot exceeds 8 MiB limit"));
    }
    tokio::fs::create_dir_all(&self.directory).await.map_err(RepositoryError::internal)?;
    super::blobs::atomic_write(&self.directory.join(format!("{id}.json")), &bytes)
      .await
      .map_err(RepositoryError::internal)?;
    Ok(id)
  }
}
