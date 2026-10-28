//! Atomic task metadata records alongside captured output.
use serde_json::Value;
use std::path::Path;
pub async fn save(directory: &Path, record: &Value) -> std::io::Result<()> {
  let temporary = directory.join("task.json.tmp");
  tokio::fs::write(&temporary, serde_json::to_vec(record)?).await?;
  tokio::fs::rename(temporary, directory.join("task.json")).await
}
pub async fn load(root: &Path) -> std::io::Result<Vec<Value>> {
  let mut entries = tokio::fs::read_dir(root).await?;
  let mut records = Vec::new();
  while let Some(entry) = entries.next_entry().await? {
    if !entry.file_type().await?.is_dir() {
      continue;
    }
    match tokio::fs::read(entry.path().join("task.json")).await {
      Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
        Ok(record) if record["task_id"].as_str().is_some_and(|id| entry.file_name() == id) => {
          records.push(record)
        }
        _ => eprintln!("ignoring invalid task record: {}", entry.path().display()),
      },
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
      Err(error) => return Err(error),
    }
  }
  Ok(records)
}

pub async fn remove(directory: &Path) -> std::io::Result<()> {
  match tokio::fs::remove_dir_all(directory).await {
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
    result => result,
  }
}
