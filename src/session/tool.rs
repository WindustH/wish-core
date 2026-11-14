//! Tool calls the model makes and the outcomes their execution settles on. Both are kept in the
//! session's state and history; the executor carries the calls out.
use crate::protocol::ContentBlock;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct ToolCall {
  pub call_id: String,
  pub name: String,
  pub arguments: Value,
}
impl ToolCall {
  /// The arguments read as `T`, or why they do not fit it, as the model is told.
  pub fn parse_arguments<T: DeserializeOwned>(&self) -> Result<T, String> {
    serde_json::from_value(self.arguments.clone()).map_err(|error| error.to_string())
  }
}

/// An unknown result is distinct from failure: an external side effect may have happened.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum ToolOutcome {
  Success(Value),
  /// Extra model input, appended after all results in this tool batch to preserve tool pairing.
  SuccessWithInput {
    output: Value,
    input: Vec<ContentBlock>,
    /// Optional model-facing result text; an empty string avoids repeating appended input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_text: Option<String>,
  },
  /// Success with application data kept on the result message and never sent to the model.
  SuccessWithMetadata {
    output: Value,
    metadata: Value,
  },
  /// Structured model presentation alongside the complete, unmodified execution result.
  SuccessWithProjection {
    output: Value,
    model_output: Value,
    metadata: Value,
  },
  /// A presentation separated from program data and UI-only metadata.
  Report {
    text: String,
    output: Value,
    metadata: Value,
    failed: bool,
  },
  Failed(String),
  Cancelled,
  Unknown(String),
}

impl ToolOutcome {
  pub(crate) fn metadata(&self) -> Value {
    match self {
      Self::Report { metadata, .. }
      | Self::SuccessWithMetadata { metadata, .. }
      | Self::SuccessWithProjection { metadata, .. } => metadata.clone(),
      _ => Value::Null,
    }
  }
  /// Durable result data. Protocol adapters send only model_text for new reports.
  pub(crate) fn encode_content(&self) -> Value {
    match self {
      Self::SuccessWithProjection { output, model_output, .. } => json!({
        "status":"success", "output":output, "model_value":{"status":"success", "output":model_output}
      }),
      Self::Success(value) | Self::SuccessWithMetadata { output: value, .. } => {
        json!({"status": "success", "output": value})
      }
      Self::SuccessWithInput { output, model_text, .. } => {
        let mut result = json!({"status":"success","output":output});
        if let Some(text) = model_text {
          result["model_text"] = json!(text);
        }
        result
      }
      Self::Report { text, output, failed, .. } => json!({
        "status": if *failed { "failed" } else { "success" },
        "output": output, "model_text": text,
      }),
      Self::Failed(message) => {
        json!({"status": "failed", "message": message, "model_text":format!("Failed: {message}")})
      }
      Self::Cancelled => {
        json!({"status": "cancelled", "model_text":"Cancelled. The operation did not complete."})
      }
      Self::Unknown(message) => {
        json!({"status": "unknown", "message": message, "model_text":format!("Outcome unknown: {message}. Inspect the resulting state before retrying; effects may already have occurred.")})
      }
    }
  }
}
