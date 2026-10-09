//! GitHub Copilot's catalog: `{data: [{id, name, vendor, capabilities, model_picker_enabled,
//! policy, ...}]}`, the models one account may pick.
//!
//! Conversions:
//! - `data[].id` is the id, `name` the name and `vendor` the owner; `capabilities.limits`
//!   gives `max_context_window_tokens` as `context_window` and `max_output_tokens` as is.
//!
//! Constraints:
//! - The body is only a list when `data` is an array, and an entry without an id is dropped.
//! - Only the chat models Copilot's own picker offers are models here: `capabilities.type` of
//!   `chat`, and `model_picker_enabled` or a `model_picker_category`. The rest are embeddings,
//!   completion engines and the snapshots the service routes to itself.
//! - The catalog does not paginate, so no page query is ever sent.
//!
//! Trade-offs:
//! - A model whose `policy.state` is not `enabled` waits for its terms to be accepted in the
//!   account's Copilot settings, and is refused until then. It stays listed, with a warning naming
//!   it, rather than accepted on the account's behalf.
//! - `supported_endpoints`, which says which wires serve a model, and the billing multipliers are
//!   not represented: this shape says what a model is called and how much it holds.

use serde_json::Value;

use crate::Error;
use crate::protocol::model_list::{Model, ModelListPage, ModelListProtocol, read_entries};

/// Reads the list body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `data` is missing or is not an array.
pub fn parse(body: &Value) -> Result<ModelListPage, Error> {
  let (entries, mut warnings) = read_entries(body, "data", "id", "copilot")?;
  let mut waiting = Vec::new();
  let mut models = Vec::new();
  for (id, entry) in entries {
    let capabilities = entry.get("capabilities");
    let chat = capabilities.and_then(|c| c.get("type")).and_then(Value::as_str) == Some("chat");
    let picked = entry.get("model_picker_enabled").and_then(Value::as_bool) == Some(true)
      || entry.get("model_picker_category").and_then(Value::as_str).is_some_and(|c| !c.is_empty());
    if !chat || !picked {
      continue;
    }
    let policy = entry.get("policy").and_then(|policy| policy.get("state")).and_then(Value::as_str);
    if policy.is_some_and(|state| state != "enabled") {
      waiting.push(id);
    }
    let limits = capabilities.and_then(|c| c.get("limits"));
    let limit = |key| limits.and_then(|limits| limits.get(key)).and_then(Value::as_u64);
    models.push(Model {
      name: entry.get("name").and_then(Value::as_str).map(str::to_owned),
      owner: entry.get("vendor").and_then(Value::as_str).map(str::to_owned),
      context_window: limit("max_context_window_tokens"),
      max_output_tokens: limit("max_output_tokens"),
      ..Model::new(id)
    });
  }
  if !waiting.is_empty() {
    warnings.push(format!(
      "enable these in the account's Copilot settings before calling them: {}",
      waiting.join(", ")
    ));
  }
  Ok(ModelListPage {
    protocol: ModelListProtocol::GitHubCopilotModels,
    models,
    next_cursor: None,
    warnings,
  })
}
