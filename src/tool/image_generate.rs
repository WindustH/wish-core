//! Image generation returns stored artifact paths, never image base64 to the language model.
use crate::{
  protocol::Tool,
  session::{ToolCall, ToolOutcome},
  tool::contract::ToolExecutor,
  utils::control::ExecutionControl,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRequest {
  pub prompt: String,
  #[serde(default)]
  pub provider: Option<String>,
}
pub type Backend = Arc<
  dyn Fn(ImageRequest) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send>> + Send + Sync,
>;
pub struct ImageGenerateTool(pub Backend, pub super::providers::Listing);
impl ImageGenerateTool {
  pub fn specification() -> Tool {
    super::providers::specification(Tool{name:"image_generate".into(),description:"Generate images using the configured image service. Describe the desired image in prompt. Returns images saved in this session's Wish blob store, including absolute local path, blob id and MIME type. Use the returned paths for further processing; do not claim to have visually inspected the images unless you read them. Generation may incur charges. If a request is interrupted or times out, do not automatically repeat it: upstream may have completed it.".into(),input_schema:json!({"type":"object","additionalProperties":false,"required":["prompt"],"properties":{"prompt":{"type":"string"},"provider":{"type":"string","description":"Optional configured image service ID; omit to use the first enabled service."}}})})
  }
}
impl ToolExecutor for ImageGenerateTool {
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let arguments =
      match super::providers::prepare(&call.arguments, &Self::specification().input_schema) {
        Ok(arguments) => arguments,
        Err(error) => return ToolOutcome::Failed(error),
      };
    if let Some(outcome) = super::providers::listing(&arguments, &self.1) {
      return outcome;
    }
    let args = match serde_json::from_value::<ImageRequest>(arguments) {
      Ok(args) => args,
      Err(error) => return ToolOutcome::Failed(error.to_string()),
    };
    match control.run_until_cancelled((self.0)(args)).await {
      None => ToolOutcome::Unknown(
        "Image request cancelled; upstream may still complete it. Do not automatically resubmit."
          .into(),
      ),
      Some(Err(e)) => ToolOutcome::Failed(e),
      Some(Ok(output)) => ToolOutcome::Report {
        text: format!("Generated images saved in Wish blob storage:\n{output}"),
        metadata: output.clone(),
        output,
        failed: false,
      },
    }
  }
}
