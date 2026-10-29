//! Stable provider selection and live discovery for service-backed tools.
use crate::{protocol::Tool, session::ToolOutcome};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Serialize)]
pub struct Provider {
  pub id: String,
  pub name: String,
  pub preset: String,
  pub model: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub kind: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub mask_editing: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub markup_editing: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub image_required: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub reference_images: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub precise_references: Option<bool>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub image_options: Option<&'static [&'static str]>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub video: Option<crate::extension_host::media::video::VideoCapabilities>,
  pub enabled: bool,
  /// Local configuration/credential readiness, not a remote health or quota check.
  pub available: bool,
}
#[derive(Serialize)]
pub struct Catalog {
  pub default_order: Vec<String>,
  pub providers: Vec<Provider>,
  pub failure_policy: &'static str,
}
pub type Listing = Arc<dyn Fn() -> Result<Catalog, String> + Send + Sync>;

pub fn specification(mut tool: Tool) -> Tool {
  tool.description.push_str(" Call with {\"action\":\"list_providers\"} to discover currently configured provider IDs and default priority, without making an upstream request. For execution, omit action or use run. Optionally set provider to an exact configured ID; this pins the request and never falls back to another provider. Otherwise configuration order determines priority. Availability is local credential readiness, not a quota or remote health check.");
  tool.input_schema.as_object_mut().unwrap().remove("required");
  tool.input_schema["properties"]["action"] =
    json!({"type":"string","enum":["run","list_providers"]});
  tool.input_schema["properties"]["provider"] = json!({"type":"string","description":"Optional exact configured provider ID. Omit to follow configuration priority."});
  tool
}

/// Validate the common envelope before either listing or executing. A listing cannot
/// silently ignore generation/search arguments, nor can an unknown action execute work.
pub fn prepare(arguments: &Value, schema: &Value) -> Result<Value, String> {
  let mut args = super::arguments::normalize(arguments, schema)?;
  let object = args.as_object_mut().ok_or("arguments must be an object")?;
  for name in object.keys() {
    if schema["properties"].get(name).is_none() {
      return Err(format!("unknown argument `{name}`"));
    }
  }
  match object.get("action").and_then(Value::as_str).unwrap_or("run") {
    "list_providers" if object.len() == 1 => return Ok(args),
    "list_providers" => {
      return Err("list_providers accepts only action; omit execution arguments".into());
    }
    "run" => {}
    action
      if schema["properties"]["action"]["enum"]
        .as_array()
        .is_some_and(|values| values.iter().any(|v| v == action)) =>
    {
      return Ok(args);
    }
    _ => return Err("unsupported action; use the tool action enum".into()),
  }
  object.remove("action");
  Ok(args)
}
pub fn listing(arguments: &Value, list: &Listing) -> Option<ToolOutcome> {
  if arguments["action"] != "list_providers" {
    return None;
  }
  Some(match list().and_then(|catalog| serde_json::to_value(catalog).map_err(|e| e.to_string())) {
    Ok(output) => {
      ToolOutcome::Report { text: output.to_string(), output, metadata: Value::Null, failed: false }
    }
    Err(error) => ToolOutcome::Failed(error),
  })
}
