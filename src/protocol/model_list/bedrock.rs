//! Bedrock's list of foundation models: `{modelSummaries: [{modelId, modelName, providerName}]}`.
//!
//! Conversions:
//! - `modelSummaries[].modelId` is the id, `modelName` the display name and `providerName` the
//!   owner; nothing in this body says how much a model holds.
//! - The answer is one page by construction, so `next_cursor` stays empty and `build_page_query`
//!   sends nothing.
//!
//! Constraints:
//! - The body is only a list when `modelSummaries` is an array, and an entry without a `modelId` is
//!   dropped: a model that cannot be called is not a model.
//!
//! Trade-offs:
//! - The service signs this request with SigV4 rather than a bearer key, exactly as the Converse
//!   wire does: the source signs the call from the account's AWS credentials, and the host the region
//!   decides - `bedrock.<region>.amazonaws.com/foundation-models` - is the caller's fact to
//!   supply.
//! - The filters the same endpoint takes (`byProvider`, `byOutputModality`, `byInferenceType`) are
//!   not sent: a caller asks for the whole list and decides what to keep.
//! - `modelLifecycle`, `inputModalities` and the rest of an entry are not represented: this shape
//!   says what a model is called and who publishes it.

use serde_json::Value;

use crate::Error;
use crate::protocol::model_list::{Model, ModelListPage, ModelListProtocol, read_entries};

/// Reads the list body.
///
/// # Errors
///
/// Returns [`Error::Malformed`] when `modelSummaries` is missing or is not an array.
pub fn parse(body: &Value) -> Result<ModelListPage, Error> {
  let (entries, warnings) = read_entries(body, "modelSummaries", "modelId", "bedrock")?;
  let models = entries
    .into_iter()
    .map(|(id, entry)| Model {
      name: entry.get("modelName").and_then(Value::as_str).map(str::to_owned),
      owner: entry.get("providerName").and_then(Value::as_str).map(str::to_owned),
      ..Model::new(id)
    })
    .collect();
  Ok(ModelListPage {
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
