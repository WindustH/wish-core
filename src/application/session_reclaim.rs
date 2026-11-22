//! Detach owned directories at deletion and coalesce disk reclamation off the request path.
use super::{
  app::App,
  error::{ApplicationError, blocking},
};
use std::{
  io::ErrorKind,
  path::{Path, PathBuf},
  sync::Arc,
  time::Duration,
};
use tokio::fs;

fn trash(app: &App) -> PathBuf {
  app.data_dir.root().join(".deleted-sessions")
}

pub(super) async fn detach(app: &App, path: &Path) -> Result<(), ApplicationError> {
  let directory = trash(app);
  fs::create_dir_all(&directory).await.map_err(ApplicationError::internal)?;
  match fs::rename(path, directory.join(uuid::Uuid::new_v4().to_string())).await {
    Ok(()) => Ok(()),
    Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
    Err(error) if error.kind() == ErrorKind::CrossesDevices => {
      fs::remove_dir_all(path).await.map_err(ApplicationError::internal)
    }
    Err(error) => Err(ApplicationError::internal(error)),
  }
}

async fn sweep(directory: &Path) -> Result<bool, std::io::Error> {
  let mut entries = match fs::read_dir(directory).await {
    Ok(entries) => entries,
    Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
    Err(error) => return Err(error),
  };
  let mut recovered = false;
  while let Some(entry) = entries.next_entry().await? {
    if uuid::Uuid::try_parse_ascii(entry.file_name().as_encoded_bytes()).is_ok() {
      let result = if entry.file_type().await?.is_dir() {
        fs::remove_dir_all(entry.path()).await
      } else {
        fs::remove_file(entry.path()).await
      };
      match result {
        Ok(()) => recovered = true,
        Err(error) => eprintln!("session trash cleanup {}: {error}", entry.path().display()),
      }
    }
  }
  Ok(recovered)
}

pub(super) fn start(app: &Arc<App>) {
  let signal = app.reclaim.clone();
  let stop = app.lifecycle.stop.clone();
  let storage = app.storage.clone();
  let directory = trash(app);
  app.lifecycle.tasks.spawn(async move {
    match sweep(&directory).await {
      Ok(true) => signal.notify_one(),
      Ok(false) => {}
      Err(error) => eprintln!("session trash recovery: {error}"),
    }
    loop {
      tokio::select! { biased; _ = stop.cancelled() => break, _ = signal.notified() => {} }
      loop {
        tokio::select! {
          biased;
          _ = stop.cancelled() => return,
          _ = signal.notified() => continue,
          _ = tokio::time::sleep(Duration::from_millis(1500)) => break,
        }
      }
      let mut first = true;
      while !stop.is_cancelled() {
        let db = storage.clone();
        match blocking(move || db.reclaim_step(first).map_err(ApplicationError::from)).await {
          Ok(true) => first = false,
          Ok(false) => {
            loop {
              let db=storage.clone();
              match blocking(move||Ok(db.finish_reclaim()?)).await {
                Ok(true)=>break,
                Ok(false)=>tokio::select!{_=stop.cancelled()=>return,_=tokio::time::sleep(Duration::from_secs(2))=>{}},
                Err(e)=>{eprintln!("session storage checkpoint: {e}");break;}
              }
            }
            break;
          }
          Err(e) => {
            eprintln!("session storage reclaim: {e}");
            break;
          }
        }
      }
      if !stop.is_cancelled()
        && let Err(error) = sweep(&directory).await
      {
        eprintln!("session trash cleanup: {error}");
      }
    }
  });
}
