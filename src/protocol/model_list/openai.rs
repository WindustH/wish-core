//! The OpenAI-compatible list: `{object, data: [{id, object, created, owned_by}]}`.
//!
//! Conversions:
//! - `data[].id` is the id, `owned_by` is `owner`, and `created` is `created_at`, read with
//!   `lexical` so a gateway writing a string does not lose the timestamp.
//! - `context_length` is read as `context_window` where a gateway adds it; the standard entry has
//!   none, and a model listed without a size is still a model.
//! - `last_id` becomes `next_cursor` when `has_more` is true, and `page_query` sends it back as
//!   `after`.
//!
//! Constraints:
//! - The body is only a list when `data` is an array, and an entry without an id is dropped: a model
//!   that cannot be called is not a model.
//! - The standard envelope does not paginate, so `after` is only ever sent once a gateway has handed
//!   a cursor over - an id on its own promises no next page.
//!
//! Trade-offs:
//! - Kimi's per-model capability flags (`supports_image_in`, `supports_video_in`,
//!   `supports_reasoning`) are reported as one warning: this shape says what a model is called and
//!   how much it holds, not what it accepts. The flags are counted into a warning, so a caller can
//!   still tell that the wire said more than the shape holds.
//! - `object` and `owned_by` are read from the entry rather than from the envelope, which is why
//!   every entry's owner is whatever its own entry said.

use serde_json::Value;

use crate::Error;
use crate::protocol::lexical;
use crate::protocol::model_list::{Model, ModelCatalog, ModelListProtocol};

/// Reads the list body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `data` is missing or is not an array.
pub fn parse(body: &Value) -> Result<ModelCatalog, Error> {
  let data = body
    .get("data")
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed("openai model list missing `data` array".to_owned()))?;
  let mut models = Vec::new();
  let mut warnings = Vec::new();
  let mut capabilities = false;
  for entry in data {
    let Some(id) = entry.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) else {
      warnings.push("an entry without an `id` was dropped".to_owned());
      continue;
    };
    for flag in ["supports_image_in", "supports_video_in", "supports_reasoning"] {
      capabilities |= entry.get(flag).is_some();
    }
    models.push(Model {
      id: id.to_owned(),
      name: None,
      owner: entry.get("owned_by").and_then(Value::as_str).map(str::to_owned),
      created_at: entry.get("created").and_then(lexical),
      context_window: entry.get("context_length").and_then(Value::as_u64),
      max_output_tokens: None,
    });
  }
  if capabilities {
    warnings.push("capability flags (`supports_image_in`, ...) are not represented".to_owned());
  }
  Ok(ModelCatalog {
    protocol: ModelListProtocol::OpenAiModels,
    models,
    next_cursor: next_cursor(body),
    warnings,
  })
}

/// The next page, when the envelope claims one: an id on its own does not.
fn next_cursor(body: &Value) -> Option<String> {
  match body.get("has_more").and_then(Value::as_bool) {
    Some(true) => body.get("last_id").and_then(Value::as_str).map(str::to_owned),
    _ => None,
  }
}

/// The query of a page: `after`, once a cursor exists to continue from.
pub(crate) fn page_query(cursor: Option<&str>) -> Vec<(String, String)> {
  cursor.map(|cursor| vec![("after".to_owned(), cursor.to_owned())]).unwrap_or_default()
}
