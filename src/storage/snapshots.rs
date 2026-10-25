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
    let bytes = tokio::fs::read(self.directory.join(format!("{id}.json")))
      .await
      .map_err(RepositoryError::internal)?;
    serde_json::from_slice(&bytes).map_err(RepositoryError::internal)
  }
  pub async fn insert<T: Serialize>(&self, value: &T) -> Result<uuid::Uuid, RepositoryError> {
    let id = uuid::Uuid::new_v4();
    let bytes = serde_json::to_vec(value).map_err(RepositoryError::internal)?;
    tokio::fs::create_dir_all(&self.directory).await.map_err(RepositoryError::internal)?;
    let temporary = self.directory.join(format!(".{id}.tmp"));
    tokio::fs::write(&temporary, bytes).await.map_err(RepositoryError::internal)?;
    if let Err(error) =
      tokio::fs::rename(&temporary, self.directory.join(format!("{id}.json"))).await
    {
      let _ = tokio::fs::remove_file(&temporary).await;
      return Err(RepositoryError::internal(error));
    }
    Ok(id)
  }
}
