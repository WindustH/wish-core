//! User-requested Library reclamation, with context and background-work ownership checks.
use super::{app::App, error::ApplicationError};
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
  #[serde(default)]
  pub archive: bool,
  pub ids: Option<HashSet<String>>,
}

#[derive(Serialize)]
pub struct CleanupResult {
  #[serde(flatten)]
  files: library::Cleanup,
  archive: Option<Value>,
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

pub(super) fn references(
  environment: BTreeMap<String, String>,
) -> impl Fn(&Message, &mut HashSet<String>) + Clone + Send + Sync + 'static {
  move |message: &Message, found: &mut HashSet<String>| {
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
  }
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
  let app = app.clone();
  let owner = owner.to_owned();
  let permit =
    if request.archive && !request.dry_run { Some(super::archives::permit()?) } else { None };
  super::error::package_work(permit.is_some(), move || {
    let _permit = permit;
    let sender = session.create_sender();
    let _frozen = sender.freeze_publication()?;
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
    let mut retained = session.resource_dependencies(references(slot.aliases.snapshot()))?;
    let unreadable_jobs = super::media_generation::cleanup::visit(&directory, |_, job| {
      if !job.terminal() {
        retained.extend(job.blob_ids());
      }
    })
    .map_err(ApplicationError::internal)?;
    let mut ids = request.ids;
    let mut archive = None;
    let package_lock;
    let _package_guard;
    if request.archive && !request.dry_run {
      let preview = library::cleanup(
        &directory,
        &retained,
        library::CleanupOptions {
          missing_links: request.missing_links,
          unused_hidden: request.unused_hidden,
          retain_all: unreadable_jobs > 0,
          ids: ids.as_ref(),
        },
        false,
      )?;
      if preview.unused_hidden + preview.missing_links > 0 {
        let mut w = super::archives::Writer::new(
          &app,
          "library_cleanup",
          Some(&owner),
          &slot.get_descriptor().name,
          None,
          false,
        )?;
        super::archives::library_files(&mut w, &directory, &preview.ids)?;
        let manifest = w.finish()?;
        package_lock = super::archives::package_lock(super::archives::package_id(&manifest)?);
        _package_guard = package_lock.lock().map_err(ApplicationError::internal)?;
        super::archives::persist(&app, &manifest, None)?;
        archive = Some(manifest);
      }
      ids = Some(preview.ids.into_iter().collect());
    }
    {
      let mut removed_file = false;
      let result = library::cleanup_tracked(
        &directory,
        &retained,
        library::CleanupOptions {
          missing_links: request.missing_links,
          unused_hidden: request.unused_hidden,
          retain_all: unreadable_jobs > 0,
          ids: ids.as_ref(),
        },
        !request.dry_run,
        &mut removed_file,
      );
      let files = match result {
        Ok(files) => files,
        Err(error) => {
          if let Some(archive) = &archive {
            if removed_file {
              return Err(ApplicationError::internal(format!(
                "library cleanup incomplete; retained archive {}: {error}",
                super::archives::package_id(archive)?
              )));
            }
            super::archives::discard_unapplied(&app, archive)?;
          }
          return Err(error.into());
        }
      };
      Ok(CleanupResult { files, unreadable_jobs, archive })
    }
  })
  .await
}
