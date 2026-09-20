//! Bedrock's list of foundation models: `{modelSummaries: [{modelId, modelName, providerName}]}`.
//!
//! Conversions:
//! - `modelSummaries[].modelId` is the id, `modelName` the display name and `providerName` the
//!   owner; nothing in this body says how much a model holds.
//! - The answer is one page by construction, so `next_cursor` stays empty and `page_query` sends
//!   nothing.
//!
//! Constraints:
//! - The body is only a list when `modelSummaries` is an array, and an entry without a `modelId` is
//!   dropped: a model that cannot be called is not a model.
//!
//! Trade-offs:
//! - The service signs this request with SigV4 rather than a bearer key, exactly as the Converse
//!   wire does: the source signs the call from the account's AWS material, and the host the region
//!   decides - `bedrock.<region>.amazonaws.com/foundation-models` - is the caller's fact to
//!   supply.
//! - The filters the same endpoint takes (`byProvider`, `byOutputModality`, `byInferenceType`) are
//!   not sent: a caller asks for the whole list and decides what to keep.
//! - `modelLifecycle`, `inputModalities` and the rest of an entry are not represented: this shape
//!   says what a model is called and who publishes it.

use serde_json::Value;

use crate::Error;
use crate::protocol::model_list::{Model, ModelCatalog, ModelListProtocol};

/// Reads the list body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `modelSummaries` is missing or is not an array.
pub fn parse(body: &Value) -> Result<ModelCatalog, Error> {
  let summaries = body.get("modelSummaries").and_then(Value::as_array).ok_or_else(|| {
    Error::Malformed("bedrock model list missing `modelSummaries` array".to_owned())
  })?;
  let mut models = Vec::new();
  let mut warnings = Vec::new();
  for entry in summaries {
    let Some(id) = entry.get("modelId").and_then(Value::as_str).filter(|id| !id.is_empty()) else {
      warnings.push("an entry without a `modelId` was dropped".to_owned());
      continue;
    };
    models.push(Model {
      id: id.to_owned(),
      name: entry.get("modelName").and_then(Value::as_str).map(str::to_owned),
      owner: entry.get("providerName").and_then(Value::as_str).map(str::to_owned),
      created_at: None,
      context_window: None,
      max_output_tokens: None,
    });
  }
  Ok(ModelCatalog {
    protocol: ModelListProtocol::BedrockModels,
    models,
    next_cursor: None,
    warnings,
  })
}

/// The query of a page: this endpoint takes no cursor and no page size.
pub(crate) fn build_page_query() -> Vec<(String, String)> {
  Vec::new()
}
