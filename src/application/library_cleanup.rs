//! User-requested Library reclamation, with context and background-work ownership checks.
use super::{
  app::App,
  error::{ApplicationError, blocking},
  session::SessionSlot,
};
use crate::{protocol::Message, storage::blobs::library};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
  collections::{BTreeMap, HashSet},
  sync::Arc,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupRequest {
  pub missing_links: bool,
  pub unused_hidden: bool,
  #[serde(default)]
  pub dry_run: bool,
  pub ids: Option<HashSet<String>>,
}

#[derive(Serialize)]
pub struct CleanupResult {
  #[serde(flatten)]
  files: library::Cleanup,
  unreadable_jobs: u64,
}

fn collect_aliases(
  value: &Value,
  environment: &BTreeMap<String, String>,
  found: &mut HashSet<String>,
) {
  match value {
    Value::String(text) => {
      for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if token.starts_with("WISH_BLOB_ID_")
          && let Some(id) = environment.get(token)
        {
          found.insert(id.clone());
        }
      }
    }
    Value::Array(items) => items.iter().for_each(|item| collect_aliases(item, environment, found)),
    Value::Object(fields) => {
      fields.values().for_each(|item| collect_aliases(item, environment, found))
    }
    _ => {}
  }
}

pub(super) fn with_dependencies<R: Send + 'static>(
  slot: &SessionSlot,
  apply: impl FnOnce(HashSet<String>) -> R + Send + 'static,
) -> Result<R, ApplicationError> {
  let environment = slot.aliases.snapshot();
  Ok(slot.sender.with_resource_dependencies(
    move |message: &Message, found| {
      if let Ok(value) = serde_json::to_value(message) {
        crate::session::resources::collect(&value, found);
        // Alias syntax examples in managed environment instructions are not file references.
        let metadata = message.get_metadata();
        let environment_notice = matches!(message, Message::System { .. })
          && (metadata["source"] == "wish_environment"
            || metadata["next_generation_instruction"]["source"] == "wish_environment");
        if !environment_notice {
          collect_aliases(&value, &environment, found);
        }
      }
    },
    apply,
  )?)
}

pub async fn cleanup(
  app: &Arc<App>,
  owner: &str,
  request: CleanupRequest,
) -> Result<CleanupResult, ApplicationError> {
  app.lifecycle.require_open()?;
  if !request.missing_links && !request.unused_hidden {
    return Err(ApplicationError::invalid("choose at least one Library cleanup option"));
  }
  if request
    .ids
    .as_ref()
    .is_some_and(|ids| ids.iter().any(|id| !crate::storage::blobs::is_blob_id(id)))
  {
    return Err(ApplicationError::invalid("invalid blob id"));
  }
  let slot = app.get_session(owner).await?;
  let session =
    slot.lock_idle().await.ok_or_else(|| ApplicationError::conflict("session is running"))?;
  let publication = slot
    .blob_publication
    .clone()
    .try_write_owned()
    .map_err(|_| ApplicationError::conflict("files are being added to this session"))?;
  let directory = app.data_dir.blobs(owner);
  blocking(move || {
    let _session = session;
    let _publication = publication;
    slot.require_live()?;
    if slot.tools.background.is_active()
      || slot.tools.tasks().is_some_and(|tasks| {
        tasks.list().map_or(true, |list| {
          list["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|task| task["process"]["status"] == "running")
        })
      })
    {
      return Err(ApplicationError::conflict("session is running"));
    }
    with_dependencies(&slot, move |mut retained| {
      let mut unreadable_jobs = 0;
      match std::fs::read_dir(directory.join(".media-jobs")) {
        Ok(entries) => {
          for entry in entries {
            let path = entry.map_err(ApplicationError::internal)?.path();
            if path.extension().is_none_or(|ext| ext != "json") {
              continue;
            }
            let job = crate::utils::bounded_file::read_sync(&path, 2 * 1024 * 1024)
              .ok()
              .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
              .filter(|job| job["phase"].is_string() && job["artifacts"].is_array());
            let Some(job) = job else {
              // Ignoring a damaged record's dependencies could erase an unfinished result.
              unreadable_jobs += 1;
              continue;
            };
            if !matches!(job["phase"].as_str(), Some("succeeded" | "failed" | "outcome_unknown")) {
              for artifact in job["artifacts"].as_array().into_iter().flatten() {
                if let Some(id) = artifact["id"].as_str() {
                  retained.insert(id.to_owned());
                }
              }
            }
          }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(ApplicationError::internal(error)),
      }
      let files = library::cleanup(
        &directory,
        &retained,
        library::CleanupOptions {
          missing_links: request.missing_links,
          unused_hidden: request.unused_hidden,
          retain_all: unreadable_jobs > 0,
          ids: request.ids.as_ref(),
        },
        !request.dry_run,
      )?;
      Ok(CleanupResult { files, unreadable_jobs })
    })?
  })
  .await
}
