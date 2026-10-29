//! Permission-aware host adapter for extension discovery and independent tool invocation.
mod audio;
mod configured;
mod fetch;
mod runway;
mod transcription;
mod video;
use super::*;
use crate::extensions::catalog::{self, Invocation, Permission, Registration};
use serde::Deserialize;
use serde_json::{Value, json};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Discovery {
  action: String,
  #[serde(default)]
  tags: Vec<String>,
  #[serde(default)]
  tool: Option<String>,
}
pub(super) fn specification() -> crate::protocol::Tool {
  crate::protocol::Tool { name: "extension_tools".into(), description: "Discover enabled extension tools by tags before inspecting their usage. search returns compact summaries; inspect returns the selected tool's full schema and instructions. Then call that named tool with its own parameters. Only currently available authorized services are listed.".into(), input_schema: json!({"type":"object","additionalProperties":false,"required":["action"],"properties":{"action":{"type":"string","enum":["search","inspect"]},"tags":{"type":"array","maxItems":16,"items":{"type":"string","maxLength":64}},"tool":{"type":"string"}}}) }
}
impl SessionTools {
  pub(super) async fn execute_extension(
    &self,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let allowed = |permission| match permission {
      Permission::ImageGeneration => self.image_generate_on.load(Ordering::Acquire),
      Permission::VideoGeneration => self.video_generate_on.load(Ordering::Acquire),
      Permission::AudioGeneration => self.audio_generate_on.load(Ordering::Acquire),
      Permission::Transcription => self.audio_transcribe_on.load(Ordering::Acquire),
      Permission::WebSearch => self.web_search_on.load(Ordering::Acquire),
      Permission::WebFetch => self.web_fetch_on.load(Ordering::Acquire),
    };
    let mut catalogs = Vec::new();
    for registration in catalog::registrations() {
      if !allowed(registration.permission)
        || catalogs.iter().any(|(permission, _)| *permission == registration.permission)
      {
        continue;
      }
      let listing = match registration.permission {
        Permission::ImageGeneration => (self.image_generate.1)(),
        Permission::VideoGeneration => {
          provider_listing(self.slot.clone(), Capability::VideoGenerate)()
        }
        Permission::AudioGeneration => {
          provider_listing(self.slot.clone(), Capability::AudioGenerate)()
        }
        Permission::Transcription => {
          provider_listing(self.slot.clone(), Capability::AudioTranscribe)()
        }
        Permission::WebSearch => provider_listing(self.slot.clone(), Capability::WebSearch)(),
        Permission::WebFetch => provider_listing(self.slot.clone(), Capability::WebFetch)(),
      };
      match listing {
        Ok(listing) => catalogs.push((registration.permission, listing)),
        Err(error) => return ToolOutcome::Failed(error),
      }
    }
    if catalogs.is_empty() {
      return ToolOutcome::Failed("extensions are disabled for this session".into());
    }
    let services_for = |registration: &Registration| {
      let Some((_, catalog)) =
        catalogs.iter().find(|(permission, _)| *permission == registration.permission)
      else {
        return Vec::new();
      };
      catalog
        .default_order
        .iter()
        .filter_map(|id| catalog.providers.iter().find(|p| &p.id == id))
        .filter(|p| {
          p.available
            && p.enabled
            && registration.service_presets.contains(&p.preset.as_str())
            && (registration.accepts_model)(p.model.as_deref().unwrap_or_default())
        })
        .map(|p| json!({"id":p.id,"name":p.name,"model":p.model,"preset":p.preset,
          "capabilities":{"video":p.video,"image_options":p.image_options,"reference_images":p.reference_images,
          "mask_editing":p.mask_editing,"markup_editing":p.markup_editing,"image_required":p.image_required}}))
        .collect::<Vec<_>>()
    };
    if call.name == catalog::DISCOVERY_TOOL {
      let args: Discovery = match call.parse_arguments() {
        Ok(v) => v,
        Err(e) => return ToolOutcome::Failed(e),
      };
      if args.tags.len() > 16 || args.tags.iter().any(|v| v.len() > 64) {
        return ToolOutcome::Failed("too many or oversized tags".into());
      }
      let output = match args.action.as_str() {
        "search" if args.tool.is_none() => {
          let tools = catalog::registrations()
            .iter()
            .filter_map(|registration| {
              let matches = args.tags.iter().all(|tag| {
                registration.tags.iter().any(|known| known.eq_ignore_ascii_case(tag.trim()))
              });
              let services = services_for(registration);
              (matches && !services.is_empty()).then(|| {
                json!({
                  "extension":registration.extension, "tool":registration.name,
                  "summary":registration.summary, "tags":registration.tags, "services":services
                })
              })
            })
            .collect::<Vec<_>>();
          json!({"tools":tools})
        }
        "inspect" if args.tags.is_empty() => {
          let Some(registration) = args.tool.as_deref().and_then(catalog::find) else {
            return ToolOutcome::Failed("unknown extension tool".into());
          };
          let services = services_for(registration);
          if services.is_empty() {
            return ToolOutcome::Failed("extension tool is unavailable".into());
          }
          json!({"extension":registration.extension,
            "tool":(registration.specification)(),
            "examples":(registration.examples)(), "services":services})
        }
        _ => return ToolOutcome::Failed("invalid discovery request".into()),
      };
      return ToolOutcome::Report {
        text: output.to_string(),
        metadata: Value::Null,
        output,
        failed: false,
      };
    }
    let Some(registration) = catalog::find(&call.name) else {
      return ToolOutcome::Failed("unknown extension tool".into());
    };
    if !allowed(registration.permission) {
      return ToolOutcome::Failed("extension is disabled for this session".into());
    }
    let invocation = match (registration.parse)(call.arguments.clone()) {
      Ok(value) => value,
      Err(error) => return ToolOutcome::Failed(error),
    };
    let invocation = match invocation {
      Invocation::Configured(arguments) => {
        return self
          .execute_configured(registration, arguments, &services_for(registration), call, control)
          .await;
      }
      Invocation::OpenaiSpeech(request) => {
        return self.execute_openai_speech(request, call, control).await;
      }
      Invocation::ElevenlabsSpeech(request) => {
        return self.execute_elevenlabs_speech(request, call, control).await;
      }
      Invocation::ElevenlabsMusic(request) => {
        return self.execute_elevenlabs_music(request, call, control).await;
      }
      Invocation::ElevenlabsSound(request) => {
        return self.execute_elevenlabs_sound(request, call, control).await;
      }
      Invocation::OpenaiTranscribe(request) => {
        return self.execute_openai_transcribe(request, control).await;
      }
      Invocation::SiliconflowTranscribe(request) => {
        return self.execute_siliconflow_transcribe(request, control).await;
      }
      Invocation::ElevenlabsTranscribe(request) => {
        return self.execute_elevenlabs_transcribe(request, control).await;
      }
      Invocation::SeedanceVideo(request) => {
        return self.execute_seedance_video(request, call, control).await;
      }
      Invocation::MinimaxVideo(request) => {
        return self.execute_minimax_video(request, call, control).await;
      }
      Invocation::LumaVideo(request) => {
        return self.execute_luma_video(request, call, control).await;
      }
      Invocation::RunninghubVideo(request) => {
        return self.execute_runninghub_video(request, call, control).await;
      }
      Invocation::XiaoyunqueVideo(request) => {
        return self.execute_xiaoyunque_video(request, call, control).await;
      }
      Invocation::JinaFetch(request) => {
        return self.execute_jina_fetch(request, &services_for(registration), control).await;
      }
      Invocation::DeepseekFetch(request) => {
        return self.execute_deepseek_fetch(request, &services_for(registration), control).await;
      }
      Invocation::OpenaiFetch(request) => {
        return self.execute_openai_fetch(request, &services_for(registration), control).await;
      }
      Invocation::ZaiFetch(request) => {
        return self.execute_zai_fetch(request, &services_for(registration), control).await;
      }
      Invocation::RunwayVideo(request) => {
        return self.execute_runway_video(request, call, control).await;
      }
      Invocation::XaiVideo(request) => {
        return self.execute_xai_video(request, call, control).await;
      }
      Invocation::XaiSpeech(request) => {
        return self.execute_xai_speech(request, call, control).await;
      }
      Invocation::XaiTranscribe(request) => {
        return self.execute_xai_transcribe(request, control).await;
      }
      Invocation::MinimaxSpeech(request) => {
        return self.execute_minimax_speech(request, call, control).await;
      }
      invocation => invocation,
    };
    use crate::extension_host::images::ImageProtocol;
    let protocol = match &invocation {
      Invocation::Novelai(_) => ImageProtocol::Novelai,
      Invocation::BflGenerate(_) | Invocation::BflFill(_) => ImageProtocol::Bfl,
      Invocation::Configured(_)
      | Invocation::JinaFetch(_)
      | Invocation::DeepseekFetch(_)
      | Invocation::OpenaiFetch(_)
      | Invocation::ZaiFetch(_)
      | Invocation::SeedanceVideo(_)
      | Invocation::MinimaxVideo(_)
      | Invocation::LumaVideo(_)
      | Invocation::RunninghubVideo(_)
      | Invocation::XiaoyunqueVideo(_)
      | Invocation::OpenaiTranscribe(_)
      | Invocation::SiliconflowTranscribe(_)
      | Invocation::ElevenlabsTranscribe(_)
      | Invocation::OpenaiSpeech(_)
      | Invocation::ElevenlabsSpeech(_)
      | Invocation::ElevenlabsMusic(_)
      | Invocation::ElevenlabsSound(_)
      | Invocation::MinimaxSpeech(_)
      | Invocation::XaiTranscribe(_)
      | Invocation::XaiSpeech(_)
      | Invocation::XaiVideo(_)
      | Invocation::RunwayVideo(_) => {
        return ToolOutcome::Failed("invalid image invocation".into());
      }
    };
    let services = services_for(registration);
    let mut request = match invocation {
      Invocation::Configured(_)
      | Invocation::JinaFetch(_)
      | Invocation::DeepseekFetch(_)
      | Invocation::OpenaiFetch(_)
      | Invocation::ZaiFetch(_)
      | Invocation::SeedanceVideo(_)
      | Invocation::MinimaxVideo(_)
      | Invocation::LumaVideo(_)
      | Invocation::RunninghubVideo(_)
      | Invocation::XiaoyunqueVideo(_)
      | Invocation::OpenaiTranscribe(_)
      | Invocation::SiliconflowTranscribe(_)
      | Invocation::ElevenlabsTranscribe(_)
      | Invocation::OpenaiSpeech(_)
      | Invocation::ElevenlabsSpeech(_)
      | Invocation::ElevenlabsMusic(_)
      | Invocation::ElevenlabsSound(_)
      | Invocation::MinimaxSpeech(_)
      | Invocation::XaiTranscribe(_)
      | Invocation::XaiSpeech(_)
      | Invocation::XaiVideo(_)
      | Invocation::RunwayVideo(_) => {
        return ToolOutcome::Failed("invalid image invocation".into());
      }
      Invocation::Novelai(args) => crate::tool::image_generate::ImageRequest {
        prompt: args.prompt,
        provider: args.service,
        images: args.source_image.into_iter().collect(),
        options: args.parameters,
        mask: None,
        markup: None,
        references: args
          .references
          .into_iter()
          .map(|reference| crate::tool::image_generate::PreciseReferenceRequest {
            image: reference.image,
            kind: reference.kind,
            strength: reference.strength,
            fidelity: reference.fidelity,
          })
          .collect(),
      },
      Invocation::BflGenerate(args) => crate::tool::image_generate::ImageRequest {
        prompt: args.prompt,
        provider: args.service,
        images: args.reference_images,
        options: args.parameters,
        mask: None,
        markup: None,
        references: Vec::new(),
      },
      Invocation::BflFill(args) => crate::tool::image_generate::ImageRequest {
        prompt: args.prompt,
        provider: args.service,
        images: vec![args.source_image],
        options: args.parameters,
        mask: args.mask,
        markup: None,
        references: Vec::new(),
      },
    };
    let selected = if let Some(id) = &request.provider {
      services.iter().find(|service| service["id"] == *id)
    } else {
      services.iter().find(|service| {
        let model = service["model"].as_str().unwrap_or_default();
        protocol.accepts_options(model, &request.options)
          && protocol.accepts_reference_count(model, request.images.len())
          && request.images.iter().all(|image| protocol.accepts_input(model, image))
          && (request.references.is_empty() || protocol.precise_references(model))
      })
    };
    let Some(id) = selected.and_then(|service| service["id"].as_str()) else {
      return ToolOutcome::Failed(
        "no enabled compatible extension service; inspect extension_tools".into(),
      );
    };
    // Pin selection before any external work. Unknown outcomes never advance to another service.
    request.provider = Some(id.to_owned());
    match control.run_until_cancelled((self.image_generate.0)(request)).await {
      Some(Ok(output)) => ToolOutcome::Report {
        text: output.to_string(),
        metadata: output.clone(),
        output,
        failed: false,
      },
      Some(Err(error)) => error.outcome(),
      None => ToolOutcome::Unknown(format!(
        "{} request interrupted; do not automatically resubmit",
        registration.extension
      )),
    }
  }
}
