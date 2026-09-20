//! Codex's catalog: `{models: [{slug, display_name, ...}]}`.
//!
//! Conversions:
//! - `models[].slug` is the id and `display_name` is the name; the rest of an entry is what the
//!   vendor's own client uses to steer a model, and none of it is a size or a date this shape holds.
//!
//! Constraints:
//! - The body is only a list when `models` is an array, and an entry without a slug is dropped: a
//!   model that cannot be called is not a model.
//! - The catalog does not paginate, so `page_query` sends only the client version the endpoint asks
//!   for and never a cursor.
//!
//! Trade-offs:
//! - The client version sent is the one the vendor's own client last reported: the endpoint asks for
//!   a caller it recognises, and this crate has no version of its own to claim.
//! - `description`, `default_reasoning_level`, `supported_reasoning_levels`, `token_budget` and
//!   `model_messages` are reported as one warning: they say how a model is asked, not what it is
//!   called or how much it holds.

use serde_json::Value;

use crate::Error;
use crate::protocol::model_list::{Model, ModelCatalog, ModelListProtocol};

/// The client version the endpoint is asked as.
const CLIENT_VERSION: &str = "0.154.0";

/// Reads the list body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `models` is missing or is not an array.
pub fn parse(body: &Value) -> Result<ModelCatalog, Error> {
  let entries = body
    .get("models")
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed("codex model list missing `models` array".to_owned()))?;
  let mut models = Vec::new();
  let mut warnings = Vec::new();
  let mut metadata = false;
  for entry in entries {
    let Some(id) = entry.get("slug").and_then(Value::as_str).filter(|id| !id.is_empty()) else {
      warnings.push("an entry without a `slug` was dropped".to_owned());
      continue;
    };
    for field in [
      "description",
      "default_reasoning_level",
      "supported_reasoning_levels",
      "token_budget",
      "model_messages",
    ] {
      metadata |= entry.get(field).is_some();
    }
    models.push(Model {
      id: id.to_owned(),
      name: entry.get("display_name").and_then(Value::as_str).map(str::to_owned),
      owner: None,
      created_at: None,
      context_window: None,
      max_output_tokens: None,
    });
  }
  if metadata {
    warnings.push(
      "model metadata (`description`, `default_reasoning_level`, ...) is not represented"
        .to_owned(),
    );
  }
  Ok(ModelCatalog {
    protocol: ModelListProtocol::OpenAiCodexModels,
    models,
    next_cursor: None,
    warnings,
  })
}

/// The query of a page: the client version, and nothing else.
pub(crate) fn build_page_query() -> Vec<(String, String)> {
  vec![("client_version".to_owned(), CLIENT_VERSION.to_owned())]
}
