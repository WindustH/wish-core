//! Cloud Code deployments become one model with selectable reasoning modes.
//! Only recognized reasoning suffixes are folded; Lite and Image remain model families.
use super::{Model, ModelListPage, ModelListProtocol};
use crate::protocol::{Request, error::Error};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn variant(id: &str) -> Option<(&str, &'static str)> {
  if !(id.starts_with("gemini-") || id.starts_with("claude-") || id.starts_with("gpt-oss-")) {
    return None;
  }
  [
    ("extra-low", "minimal"),
    ("minimal", "minimal"),
    ("low", "low"),
    ("medium", "medium"),
    ("high", "high"),
    ("xhigh", "xhigh"),
    ("max", "max"),
    ("thinking", "enabled"),
    ("tiered", "adaptive"),
  ]
  .into_iter()
  .find_map(|(suffix, effort)| id.strip_suffix(&format!("-{suffix}")).map(|base| (base, effort)))
}

pub fn parse(body: &Value) -> Result<ModelListPage, Error> {
  let entries = body
    .get("models")
    .and_then(Value::as_object)
    .ok_or_else(|| Error::Malformed("Antigravity model list has no models object".into()))?;
  let mut groups = BTreeMap::<&str, Vec<(&str, &Value, Option<&str>)>>::new();
  for (id, value) in entries {
    if id.is_empty() || id.starts_with("tab_") || id.starts_with("chat_") {
      continue;
    }
    let (base, effort) =
      variant(id).map_or((id.as_str(), None), |(base, effort)| (base, Some(effort)));
    groups.entry(base).or_default().push((id, value, effort));
  }
  let mut models = Vec::new();
  for (id, entries) in groups {
    let grouped = entries.iter().any(|(_, _, effort)| effort.is_some());
    let mut model = Model::new(id);
    if grouped {
      for (wire_id, value, effort) in &entries {
        let effort =
          effort.unwrap_or(if value["supportsThinking"] == false { "none" } else { "default" });
        model.reasoning_variants.insert(effort.to_owned(), (*wire_id).to_owned());
      }
      model.default_reasoning_effort = [
        "default", "medium", "high", "low", "adaptive", "none", "enabled", "minimal", "xhigh",
        "max",
      ]
      .into_iter()
      .find(|effort| model.reasoning_variants.contains_key(*effort))
      .map(str::to_owned);
    }
    let selected = model
      .default_reasoning_effort
      .as_ref()
      .and_then(|effort| model.reasoning_variants.get(effort));
    let (_, value, _) = entries
      .iter()
      .find(|(wire_id, _, _)| selected.is_some_and(|selected| selected == wire_id))
      .unwrap_or(&entries[0]);
    model.name = value["displayName"].as_str().map(|name| {
      if grouped
        && let Some((base, suffix)) = name.rsplit_once(" (")
        && [
          "low)",
          "medium)",
          "high)",
          "extra-low)",
          "minimal)",
          "xhigh)",
          "max)",
          "thinking)",
          "tiered)",
        ]
        .contains(&suffix.to_ascii_lowercase().as_str())
      {
        base.to_owned()
      } else {
        name.to_owned()
      }
    });
    // Use a limit safe for every advertised mode when the service reports different limits.
    model.context_window = entries
      .iter()
      .filter_map(|(_, v, _)| {
        v.get("maxInputTokens").or_else(|| v.get("maxTokens")).and_then(Value::as_u64)
      })
      .min();
    model.max_output_tokens =
      entries.iter().filter_map(|(_, v, _)| v["maxOutputTokens"].as_u64()).min();
    model.supports_reasoning = Some(grouped || value["supportsThinking"] == true);
    models.push(model);
  }
  Ok(ModelListPage {
    protocol: ModelListProtocol::GoogleAntigravityModels,
    models,
    next_cursor: None,
    warnings: Vec::new(),
  })
}

/// Resolve only deployments actually advertised by this account. Legacy callable ids
/// remain accepted, so stored conversations do not change their original tier.
pub fn resolve(request: &Request, models: &[Model]) -> Result<Request, Error> {
  let mut resolved = request.clone();
  let Some(model) = models.iter().find(|model| model.id == request.model) else {
    return Ok(resolved);
  };
  if model.reasoning_variants.is_empty() {
    return Ok(resolved);
  }
  let reasoning = request.reasoning.as_ref();
  let explicit = reasoning.and_then(|r| r.effort.as_deref());
  let effort = if reasoning.and_then(|r| r.enabled) == Some(false) {
    "none"
  } else {
    match explicit {
      Some("none" | "off") => "none",
      Some("default") if model.reasoning_variants.contains_key("default") => "default",
      None | Some("auto" | "default") => {
        model.default_reasoning_effort.as_deref().unwrap_or("default")
      }
      Some(effort) => effort,
    }
  };
  resolved.model = model
    .reasoning_variants
    .get(effort)
    .ok_or_else(|| {
      Error::Build(format!(
        "Antigravity model {} has no {effort} reasoning deployment; available: {}",
        model.id,
        model.reasoning_variants.keys().cloned().collect::<Vec<_>>().join(", ")
      ))
    })?
    .clone();
  if matches!(effort, "enabled" | "adaptive" | "default") {
    resolved.reasoning = None;
  } else if effort == "none" {
    resolved.reasoning = Some(crate::protocol::model_use::request::ReasoningConfig {
      enabled: Some(false),
      effort: None,
      summary: request.reasoning.as_ref().and_then(|r| r.summary),
    });
  }
  Ok(resolved)
}

/// Configured overrides use the same public model ids as the catalog. A legacy tier's
/// metadata must not hide the other modes now supplied by that catalog.
pub fn normalize_overrides(values: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
  let mut models: BTreeMap<_, _> = values
    .iter()
    .filter(|(id, _)| variant(id).is_none())
    .map(|(k, v)| (k.clone(), v.clone()))
    .collect();
  for (id, value) in values {
    let Some((base, effort)) = variant(id) else { continue };
    let mut value = value.clone();
    if let Some(object) = value.as_object_mut() {
      object.remove("reasoning_efforts");
      object.remove("fixed_reasoning_effort");
      object.insert("default_reasoning_effort".into(), json!(effort));
    }
    models.entry(base.to_owned()).or_insert(value);
  }
  models
}
