//! Scoped resource access. Supplier wire formats and parameter conversion stay in extensions.
use super::*;
use crate::extensions::{
  image_runtime::ImageRequest,
  invocation::{Failure, Future, Host, Read, Report},
  media_runtime::MediaRequest,
  shared::web_search::SearchQuery,
  transcription_runtime::Transcription,
};
pub(super) struct Adapter<'a> {
  pub tools: &'a SessionTools,
  pub registration: &'static Registration,
  pub services: Vec<Value>,
  pub call_id: &'a str,
}
impl Adapter<'_> {
  fn require(&self, permission: Permission) -> Result<(), Failure> {
    if self.registration.permission == permission {
      Ok(())
    } else {
      Err("extension facility not authorized".into())
    }
  }
  fn select(&self, requested: Option<&str>) -> Result<String, Failure> {
    self
      .services
      .iter()
      .find(|s| requested.is_none_or(|id| s["id"] == id))
      .and_then(|s| s["id"].as_str())
      .map(str::to_owned)
      .ok_or_else(|| "no enabled service for this extension tool".into())
  }
}
fn paid(error: crate::tool::paid_operation::Failure) -> Failure {
  match error {
    crate::tool::paid_operation::Failure::Rejected(e) => Failure::Rejected(e),
    crate::tool::paid_operation::Failure::Unknown(e) => Failure::Unknown(format!(
      "{e}; upstream may have processed the request; do not automatically resubmit"
    )),
  }
}
impl Host for Adapter<'_> {
  fn image(&self, mut request: ImageRequest) -> Future<'_> {
    Box::pin(async move {
      self.require(Permission::ImageGeneration)?;
      let selected = self.services.iter().find(|service| {
        if let Some(id) = &request.provider {
          return service["id"] == *id;
        }
        let Some(preset) = crate::application::image_generation::presets()
          .iter()
          .find(|p| service["preset"] == p.id)
        else {
          return false;
        };
        let model = service["model"].as_str().unwrap_or_default();
        let protocol = preset.protocol;
        protocol.accepts_options(model, &request.options)
          && protocol.accepts_reference_count(model, request.images.len())
          && request.images.iter().all(|image| protocol.accepts_input(model, image))
          && (request.references.is_empty() || protocol.precise_references(model))
          && (request.mask.is_none() || protocol.mask_editing(model))
          && (request.markup.is_none() || protocol.markup_editing(model))
      });
      request.provider = Some(
        selected
          .and_then(|s| s["id"].as_str())
          .ok_or("no enabled compatible extension service")?
          .to_owned(),
      );
      (self.tools.image_generate.0)(request).await.map(Report::json).map_err(paid)
    })
  }
  fn media(&self, request: MediaRequest) -> Future<'_> {
    Box::pin(async move {
      let video = match self.registration.permission {
        Permission::VideoGeneration => true,
        Permission::AudioGeneration => false,
        _ => return Err("extension facility not authorized".into()),
      };
      let owner = self.tools.slot.upgrade().ok_or("session is closed")?;
      let app = owner.app.upgrade().ok_or("server is closed")?;
      let accounts = app.get_service_accounts();
      let hub = if video { &app.videos } else { &app.audio };
      hub
        .execute_scoped(
          request,
          self.call_id,
          &owner.blob_dir,
          &accounts,
          crate::application::media_generation::Scope {
            video,
            presets: self.registration.service_presets,
          },
        )
        .await
        .map(Report::json)
        .map_err(Failure::Rejected)
    })
  }
  fn transcribe(&self, request: Box<dyn Transcription>) -> Future<'_> {
    Box::pin(async move {
      self.require(Permission::Transcription)?;
      if request.registration().name != self.registration.name {
        return Err("extension tool identity mismatch".into());
      }
      let owner = self.tools.slot.upgrade().ok_or("session is closed")?;
      let app = owner.app.upgrade().ok_or("server is closed")?;
      let accounts = app.get_service_accounts();
      app
        .asr
        .transcribe_scoped(request, &owner.blob_dir, &accounts)
        .await
        .map(Report::json)
        .map_err(paid)
    })
  }
  fn read(&self, request: Read) -> Future<'_> {
    Box::pin(async move {
      self.require(Permission::WebFetch)?;
      let allowed: Vec<_> = self.services.iter().filter_map(|s| s["id"].as_str()).collect();
      let provider = if request.cursor.as_deref().is_some_and(|v| !v.trim().is_empty()) {
        request.service.as_deref()
      } else {
        request.service.as_deref().or_else(|| allowed.first().copied())
      };
      if allowed.is_empty() || provider.is_some_and(|p| !allowed.contains(&p)) {
        return Err("no enabled service for this extension reader".into());
      }
      let page = self
        .tools
        .web_fetch
        .read(
          request.url.as_deref(),
          request.cursor.as_deref(),
          request.refresh,
          provider,
          &allowed,
          self.registration.extension,
        )
        .await?;
      Ok(Report {
        text: crate::application::fetch::reader::render(&page.output, self.registration.name),
        output: page.output,
        metadata: page.metadata,
      })
    })
  }
  fn search(&self, query: SearchQuery, service: Option<String>) -> Future<'_> {
    Box::pin(async move {
      self.require(Permission::WebSearch)?;
      let provider = self.select(service.as_deref())?;
      Ok(self.tools.web_search.run(query, Some(provider)).await?)
    })
  }
}
