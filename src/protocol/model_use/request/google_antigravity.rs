//! Antigravity serves Gemini contents inside a Cloud Code envelope. Account and
//! session fields join the rendered request in the client, where both are available.
use crate::protocol::{Request, error::Error};
use serde_json::{Value, json};

pub fn render(request: &Request) -> Result<Value, Error> {
  let mut normalized = request.clone();
  if crate::protocol::model_list::antigravity::variant(&request.model).is_some() {
    normalized.reasoning = None;
  }
  let mut body = super::google_generate_content::render(&normalized)?;
  if let Some(declarations) =
    body.pointer_mut("/tools/0/functionDeclarations").and_then(Value::as_array_mut)
  {
    for declaration in declarations {
      declaration["parameters"] = render_schema(&declaration["parameters"])?;
    }
  }
  // Cloud Code's Claude deployments accept a thinking budget, not Gemini's depth word.
  if let Some(effort) = fixed_effort(&request.model) {
    if body.get("generationConfig").is_none() {
      body["generationConfig"] = json!({});
    }
    // A named deployment owns its tier. Stale session settings cannot turn a High
    // deployment into Low, nor disable its required thinking.
    body["generationConfig"]["thinkingConfig"] = if request.model.starts_with("gpt-oss-") {
      json!({"includeThoughts": true})
    } else {
      json!({"thinkingLevel": effort.to_uppercase(), "includeThoughts": true})
    };
  } else if request.model.ends_with("-tiered") {
    body["generationConfig"] = body.get("generationConfig").cloned().unwrap_or_else(|| json!({}));
    body["generationConfig"]["thinkingConfig"] = json!({"includeThoughts": true});
  } else if request.model.contains("claude") {
    let enabled = request.reasoning.as_ref().and_then(|r| r.enabled);
    if enabled != Some(false) && (request.model.contains("thinking") || request.reasoning.is_some())
    {
      let effort = request.reasoning.as_ref().and_then(|r| r.effort.as_deref());
      let budget = match effort {
        None | Some("high") => 16384,
        Some("low") => 1024,
        Some("medium") => 8192,
        Some("max") => 32768,
        Some(value) => {
          return Err(Error::Build(format!(
            "unsupported Antigravity Claude thinking effort: {value}"
          )));
        }
      };
      if body.get("generationConfig").is_none() {
        body["generationConfig"] = json!({});
      }
      body["generationConfig"]["thinkingConfig"] =
        json!({"thinkingBudget": budget, "includeThoughts": true});
      let limit = request.max_output_tokens.unwrap_or(65536);
      if limit <= budget {
        return Err(Error::Build(
          "Antigravity Claude output limit must exceed its thinking budget".to_owned(),
        ));
      }
      body["generationConfig"]["maxOutputTokens"] = json!(limit);
    }
  } else if request.model.ends_with("-thinking") {
    body["generationConfig"] = body.get("generationConfig").cloned().unwrap_or_else(|| json!({}));
    body["generationConfig"]["thinkingConfig"] = json!({"includeThoughts": true});
  }
  Ok(body)
}

/// Cloud Code accepts Google's Schema message, not arbitrary JSON Schema keywords.
/// Visit schema nodes only: a property named `type` or `default` is still a property.
fn render_schema(schema: &Value) -> Result<Value, Error> {
  let object = schema
    .as_object()
    .ok_or_else(|| Error::Build("Antigravity tool schemas must be objects".to_owned()))?;
  if ["$ref", "oneOf", "allOf", "not"].iter().any(|key| object.contains_key(*key)) {
    return Err(Error::Build(
      "Antigravity tool schemas cannot contain references, oneOf, allOf or not".to_owned(),
    ));
  }
  let mut result = serde_json::Map::new();
  for (key, value) in object {
    let rendered = match key.as_str() {
      "type" => {
        let kind = value.as_str().ok_or_else(|| {
          Error::Build("Antigravity schema type must be a single type".to_owned())
        })?;
        json!(kind.to_uppercase())
      }
      "properties" => {
        let properties = value
          .as_object()
          .ok_or_else(|| Error::Build("schema properties must be an object".to_owned()))?;
        Value::Object(
          properties
            .iter()
            .map(|(name, schema)| Ok((name.clone(), render_schema(schema)?)))
            .collect::<Result<_, Error>>()?,
        )
      }
      "items" => render_schema(value)?,
      "anyOf" => Value::Array(
        value
          .as_array()
          .ok_or_else(|| Error::Build("schema anyOf must be an array".to_owned()))?
          .iter()
          .map(render_schema)
          .collect::<Result<_, _>>()?,
      ),
      "format" | "description" | "nullable" | "enum" | "required" | "minimum" | "maximum"
      | "minItems" | "maxItems" | "minProperties" | "maxProperties" | "propertyOrdering" => {
        value.clone()
      }
      _ => continue,
    };
    result.insert(key.clone(), rendered);
  }
  Ok(Value::Object(result))
}

pub fn wrap(
  body: Value,
  model: &str,
  project: Option<&str>,
  session: &str,
) -> Result<Value, Error> {
  let project = project.filter(|v| !v.is_empty()).ok_or_else(|| {
    Error::Build("Antigravity requires a Cloud Code project; sign in with Google first".to_owned())
  })?;
  let mut body = body;
  body["sessionId"] = json!(session);
  Ok(json!({
    "project": project, "model": model, "request": body,
    "userAgent": "antigravity", "requestType": "agent",
    "requestId": format!("agent-{}", uuid::Uuid::new_v4()),
  }))
}

/// Named Cloud Code deployments expose their tier in the callable id.
pub fn fixed_effort(model: &str) -> Option<&'static str> {
  crate::protocol::model_list::antigravity::variant(model)
    .map(|(_, effort)| effort)
    .filter(|effort| !matches!(*effort, "enabled" | "adaptive"))
}
