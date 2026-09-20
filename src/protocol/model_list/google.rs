//! Gemini's model list: `{models: [{name, displayName, inputTokenLimit, ...}], nextPageToken}`.
//!
//! Conversions:
//! - `name` is a resource get_name (`models/gemini-2.5-flash`) and loses the `models/` prefix, because a
//!   call passes the bare id back.
//! - `displayName` becomes `name`, `inputTokenLimit` `context_window` and `outputTokenLimit`
//!   `max_output_tokens`.
//! - `nextPageToken` becomes `next_cursor` while it is non-empty, and `page_query` sends it back as
//!   `pageToken` beside the `pageSize` the wire wants spelled out.
//!
//! Constraints:
//! - The body is only a list when `models` is an array, and an entry without a `name` is dropped.
//!
//! Trade-offs:
//! - The sampling parameters a model entry carries (`temperature`, `topP`, `topK`) are reported as
//!   one warning: they are defaults of the list endpoint rather than a description of the model.
//! - No entry carries an owner or a creation time, so both stay empty.

use serde_json::Value;

use crate::Error;
use crate::protocol::model_list::{Model, ModelCatalog, ModelListProtocol};

/// The prefix a resource name carries, which is not part of the id.
const RESOURCE_PREFIX: &str = "models/";

/// Reads one page.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `models` is missing or is not an array.
pub fn parse(body: &Value) -> Result<ModelCatalog, Error> {
  let entries = body
    .get("models")
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed("google model list missing `models` array".to_owned()))?;
  let mut models = Vec::new();
  let mut warnings = Vec::new();
  let mut tuning = false;
  for entry in entries {
    let Some(name) = entry.get("name").and_then(Value::as_str).filter(|name| !name.is_empty())
    else {
      warnings.push("an entry without a `name` was dropped".to_owned());
      continue;
    };
    for field in ["temperature", "topP", "topK"] {
      tuning |= entry.get(field).is_some();
    }
    models.push(Model {
      // A resource name may carry `models/` (Gemini API) or `publishers/{publisher}/models/`
      // (Vertex): strip whichever prefix it has, since a call passes the bare id back.
      id: {
        let rest = name.strip_prefix("publishers/").unwrap_or(name);
        let rest = rest.split_once("/models/").map(|(_, id)| id).unwrap_or(rest);
        rest.strip_prefix(RESOURCE_PREFIX).unwrap_or(rest).to_owned()
      },
      name: entry.get("displayName").and_then(Value::as_str).map(str::to_owned),
      owner: None,
      created_at: None,
      context_window: entry.get("inputTokenLimit").and_then(Value::as_u64),
      max_output_tokens: entry.get("outputTokenLimit").and_then(Value::as_u64),
    });
  }
  if tuning {
    warnings
      .push("sampling parameters (`temperature`, `topP`, `topK`) are not represented".to_owned());
  }
  let next_cursor = body
    .get("nextPageToken")
    .and_then(Value::as_str)
    .filter(|token| !token.is_empty())
    .map(str::to_owned);
  Ok(ModelCatalog { protocol: ModelListProtocol::GoogleModels, models, next_cursor, warnings })
}

/// The query of a page: an opaque token, and the page size the wire wants spelled out.
pub(crate) fn build_page_query(cursor: Option<&str>, page_size: u32) -> Vec<(String, String)> {
  let mut query = Vec::new();
  if let Some(cursor) = cursor {
    query.push(("pageToken".to_owned(), cursor.to_owned()));
  }
  query.push(("pageSize".to_owned(), page_size.to_string()));
  query
}
