//! The DashScope catalog: `{total, page_no, page_size, data: [{id, model_name, ...}]}`.
//!
//! Conversions:
//! - `id` is the dated snapshot (`qwen-max-2025-01-25`) and stays the id; `model_name` is the
//!   family and becomes `name`, which is what a caller shows.
//! - `context_window` and `max_output_tokens` are read where an entry carries them.
//! - Pagination is by page number: the cursor is the next page's number, computed from `page_no`
//!   times `page_size` against `total`, and `page_query` spells the first page `page_no=1` rather
//!   than leaving it out.
//!
//! Constraints:
//! - The body is only a list when `data` is an array, and an entry without an id is dropped.
//! - An envelope missing any one of `page_no`, `page_size` and `total` is the last page: the
//!   arithmetic is the wire's own rule, and guessing a part of it would read a page twice or skip
//!   one.
//!
//! Trade-offs:
//! - The entry's `description` and the rest of the catalog metadata are not represented.
//! - The dated snapshot is the id and the family the name, so calling by the family is the caller's
//!   mapping rather than a field here.

use serde_json::Value;

use crate::Error;
use crate::protocol::model_list::{Model, ModelCatalog, ModelListProtocol};

/// Reads one page.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `data` is missing or is not an array.
pub fn parse(body: &Value) -> Result<ModelCatalog, Error> {
  let data = body
    .get("data")
    .and_then(Value::as_array)
    .ok_or_else(|| Error::Malformed("qwen model list missing `data` array".to_owned()))?;
  let mut models = Vec::new();
  let mut warnings = Vec::new();
  for entry in data {
    let Some(id) = entry.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) else {
      warnings.push("an entry without an `id` was dropped".to_owned());
      continue;
    };
    models.push(Model {
      id: id.to_owned(),
      name: entry.get("model_name").and_then(Value::as_str).map(str::to_owned),
      owner: None,
      created_at: None,
      context_window: entry.get("context_window").and_then(Value::as_u64),
      max_output_tokens: entry.get("max_output_tokens").and_then(Value::as_u64),
    });
  }
  // The next page number, while the total says there is one: the arithmetic is the wire's own rule
  // rather than a guess, so no page is read twice and none is skipped.
  let next_cursor = (|| {
    let page = body.get("page_no").and_then(Value::as_u64)?;
    let size = body.get("page_size").and_then(Value::as_u64)?;
    let total = body.get("total").and_then(Value::as_u64)?;
    (page * size < total).then(|| (page + 1).to_string())
  })();
  Ok(ModelCatalog { protocol: ModelListProtocol::QwenModels, models, next_cursor, warnings })
}

/// The query of a page: an explicit page number, because the first page is page one.
pub(crate) fn page_query(cursor: Option<&str>, page_size: u32) -> Vec<(String, String)> {
  vec![
    ("page_no".to_owned(), cursor.unwrap_or("1").to_owned()),
    ("page_size".to_owned(), page_size.to_string()),
  ]
}
