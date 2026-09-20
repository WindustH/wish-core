//! Anthropic's model list: `{data: [{type, id, display_name, created_at}], has_more, last_id}`.
//!
//! Conversions:
//! - `id` stays the id and `display_name` becomes `name`.
//! - `created_at` is an ISO instant here rather than a unix second; it is kept exactly as the wire
//!   wrote it, because a timestamp this shape does not convert is still a usable timestamp.
//! - Cursors run both ways, and this reader follows the forward one: `last_id` becomes `next_cursor`
//!   while `has_more` is true, and `page_query` sends it back as `after_id` beside the `limit` the
//!   wire wants spelled out.
//!
//! Constraints:
//! - The body is only a list when `data` is an array, and an entry without an id is dropped.
//! - The API is dated in a header rather than in the body, and that header belongs to the source
//!   table: nothing here can date the read.
//!
//! Trade-offs:
//! - The entry's `type` and the backward cursor (`first_id`, `before_id`) are not represented: this
//!   shape reads a list forward.
//! - No entry carries a context window, so `context_window` and `max_output_tokens` stay empty
//!   rather than being assumed.

use serde_json::Value;

use crate::Error;
use crate::protocol::model_list::{Model, ModelCatalog, ModelListProtocol};
use crate::protocol::read_scalar_text;

/// Reads one page.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `data` is missing or is not an array.
pub fn parse(body: &Value) -> Result<ModelCatalog, Error> {
  let data = body
    .get("data")
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed("anthropic model list missing `data` array".to_owned()))?;
  let mut models = Vec::new();
  let mut warnings = Vec::new();
  for entry in data {
    let Some(id) = entry.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) else {
      warnings.push("an entry without an `id` was dropped".to_owned());
      continue;
    };
    models.push(Model {
      id: id.to_owned(),
      name: entry.get("display_name").and_then(Value::as_str).map(str::to_owned),
      owner: None,
      created_at: entry.get("created_at").and_then(read_scalar_text),
      context_window: None,
      max_output_tokens: None,
    });
  }
  let next_cursor = match body.get("has_more").and_then(Value::as_bool) {
    Some(true) => body.get("last_id").and_then(Value::as_str).map(str::to_owned),
    _ => None,
  };
  Ok(ModelCatalog { protocol: ModelListProtocol::AnthropicModels, models, next_cursor, warnings })
}

/// The query of a page: the forward cursor, and the page size the wire wants spelled out.
pub(crate) fn build_page_query(cursor: Option<&str>, page_size: u32) -> Vec<(String, String)> {
  let mut query = Vec::new();
  if let Some(cursor) = cursor {
    query.push(("after_id".to_owned(), cursor.to_owned()));
  }
  query.push(("limit".to_owned(), page_size.to_string()));
  query
}
