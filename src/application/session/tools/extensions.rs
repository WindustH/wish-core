//! Permission-aware host adapter for extension discovery and independent tool invocation.
mod host;
use super::*;
use crate::extensions::catalog::{self, Permission, Registration};
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
    let host = host::Adapter {
      tools: self,
      registration,
      services: services_for(registration),
      call_id: &call.call_id,
    };
    match control.run_until_cancelled(invocation.execute(&host)).await {
      Some(Ok(report)) => ToolOutcome::Report {
        text: report.text,
        output: report.output,
        metadata: report.metadata,
        failed: false,
      },
      Some(Err(crate::extensions::invocation::Failure::Rejected(error))) => {
        ToolOutcome::Failed(error)
      }
      Some(Err(crate::extensions::invocation::Failure::Unknown(error))) => {
        ToolOutcome::Unknown(error)
      }
      None if matches!(registration.permission, Permission::WebSearch | Permission::WebFetch) => {
        ToolOutcome::Cancelled
      }
      None => ToolOutcome::Unknown(format!(
        "{} interrupted; inspect existing jobs before resubmitting",
        registration.name
      )),
    }
  }
}
