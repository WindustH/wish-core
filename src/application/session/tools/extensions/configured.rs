//! Adapts validated, extension-owned arguments to host persistence and execution services.
use super::*;

impl SessionTools {
  pub(super) async fn execute_configured(
    &self,
    registration: &'static Registration,
    arguments: Value,
    services: &[Value],
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let Some(mut arguments) = arguments.as_object().cloned() else {
      return ToolOutcome::Failed("expected tool arguments object".into());
    };
    arguments.retain(|_, value| !value.is_null());
    let requested = arguments.remove("service");
    let selected = if let Some(id) = requested.as_ref() {
      services.iter().find(|service| &service["id"] == id)
    } else {
      services.iter().find(|service| {
        if registration.permission != Permission::ImageGeneration {
          return true;
        }
        let Some(preset) = crate::application::image_generation::presets()
          .iter()
          .find(|preset| service["preset"] == preset.id)
        else {
          return false;
        };
        let model = service["model"].as_str().unwrap_or_default();
        let options = arguments.get("parameters").and_then(Value::as_object);
        let images = arguments.get("reference_images").and_then(Value::as_array);
        options.is_none_or(|options| preset.protocol.accepts_options(model, options))
          && preset.protocol.accepts_reference_count(model, images.map_or(0, Vec::len))
          && images.is_none_or(|images| {
            images.iter().all(|image| {
              image.as_str().is_some_and(|image| preset.protocol.accepts_input(model, image))
            })
          })
          && (!arguments.contains_key("mask") || preset.protocol.mask_editing(model))
          && (!arguments.contains_key("markup") || preset.protocol.markup_editing(model))
      })
    };
    let Some(selected) = selected else {
      return ToolOutcome::Failed(
        "no enabled service for this extension tool; inspect it before calling".into(),
      );
    };
    // Selection is constrained to this extension before any external request.
    arguments.insert("provider".into(), selected["id"].clone());
    for (from, to) in [("parameters", "options"), ("reference_images", "images")] {
      if let Some(value) = arguments.remove(from) {
        arguments.insert(to.into(), value);
      }
    }
    let arguments = Value::Object(arguments);
    let adapted = ToolCall { arguments: arguments.clone(), ..call.clone() };
    match registration.permission {
      Permission::WebSearch => self.web_search.execute(&adapted, control).await,
      Permission::WebFetch => {
        ToolOutcome::Failed("reader requires an extension-owned invocation".into())
      }
      Permission::Transcription => {
        ToolOutcome::Failed("transcription requires an extension-owned invocation".into())
      }
      Permission::ImageGeneration => {
        let request = match serde_json::from_value(arguments) {
          Ok(request) => request,
          Err(error) => return ToolOutcome::Failed(error.to_string()),
        };
        match control.run_until_cancelled((self.image_generate.0)(request)).await {
          Some(Ok(output)) => report(output),
          Some(Err(error)) => error.outcome(),
          None => {
            ToolOutcome::Unknown("image request interrupted; do not automatically resubmit".into())
          }
        }
      }
      Permission::VideoGeneration | Permission::AudioGeneration => {
        ToolOutcome::Failed("media requires an extension-owned invocation".into())
      }
    }
  }
}
fn report(output: Value) -> ToolOutcome {
  ToolOutcome::Report { text: output.to_string(), metadata: output.clone(), output, failed: false }
}
