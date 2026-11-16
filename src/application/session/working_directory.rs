//! Recover a removed checkout without losing the session or silently changing its environment.
use super::SessionSlot;
use crate::application::error::{ApplicationError, blocking};
use std::{io::ErrorKind, path::PathBuf, sync::Arc};

impl SessionSlot {
  /// Called before model turns and process starts, and while reopening a stored session.
  /// Serializes with explicit cwd changes; only one caller publishes the migration notice.
  pub(crate) async fn ensure_working_directory(self: &Arc<Self>) -> Result<bool, ApplicationError> {
    let cwd = self.get_descriptor().cwd;
    match tokio::fs::metadata(&cwd).await {
      Ok(metadata) if metadata.is_dir() => return Ok(false),
      Ok(_) => {}
      Err(error) if error.kind() == ErrorKind::NotFound => {}
      Err(error) => {
        return Err(ApplicationError::invalid(format!(
          "cannot access session working directory {}: {error}",
          cwd.display()
        )));
      }
    }
    let app = self.app.upgrade().ok_or_else(ApplicationError::shutting_down)?;
    // Do not hold the config lock while waiting for session mutations or filesystem work.
    let fallback = app.config_file.lock().await.config.defaults.cwd.clone();
    let slot = self.clone();
    let _update = self.update_lock.clone().lock_owned().await;
    blocking(move || {
      slot.require_live()?;
      let previous = slot.get_descriptor().cwd;
      match std::fs::metadata(&previous) {
        Ok(metadata) if metadata.is_dir() => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(ApplicationError::invalid(error.to_string())),
      }
      let next = std::fs::canonicalize(&fallback)
        .ok()
        .filter(|path| path.is_dir())
        .ok_or_else(|| ApplicationError::invalid(format!(
          "session working directory {} no longer exists as a directory; default working directory {} is also unavailable. Set an existing working directory in session settings",
          previous.display(), fallback.display()
        )))?;
      let notice = format!(
        "The session working directory {} no longer exists as a directory. The current working directory has been migrated to the default working directory {}. Use the new directory for subsequent commands.",
        serde_json::json!(previous), serde_json::json!(next)
      );
      slot.apply_working_directory(next, Some(notice))?;
      Ok(true)
    }).await
  }

  pub fn set_working_directory(&self, cwd: PathBuf) -> Result<(), ApplicationError> {
    let cwd =
      std::fs::canonicalize(cwd).map_err(|error| ApplicationError::invalid(error.to_string()))?;
    if !cwd.is_dir() {
      return Err(ApplicationError::invalid("cwd must be a directory"));
    }
    self.apply_working_directory(cwd, None)
  }

  fn apply_working_directory(
    &self,
    cwd: PathBuf,
    notice: Option<String>,
  ) -> Result<(), ApplicationError> {
    if self.get_descriptor().cwd == cwd {
      return Ok(());
    }
    self.edit_descriptor(None, |descriptor| {
      descriptor.cwd = cwd.clone();
      Ok(())
    })?;
    self.tools.update_working_directory(cwd.clone());
    if let Some(notice) = notice {
      let text = crate::application::instructions::environment_instruction(
        &self.effective_shell.read().unwrap(),
        &cwd,
        self.get_descriptor().temp_dir.as_deref(),
      );
      self.sender.enqueue_instruction_update(
        "wish_environment".into(),
        text.clone(),
        format!("{notice}\n{text}"),
      )?;
      let _ = self.events.send(serde_json::json!({"type":"history_changed"}));
      Ok(())
    } else {
      self.environment_notice(&self.effective_shell.read().unwrap(), &cwd)
    }
  }
}
