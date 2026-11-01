//! Read a local image as native model input, without modifying the source file.
use crate::{
  protocol::{ContentBlock, Tool},
  session::{ToolCall, ToolOutcome},
  tool::contract::ToolExecutor,
  utils::control::ExecutionControl,
};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

#[derive(Clone, Default)]
pub struct ViewImageTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
  path: PathBuf,
}
impl ViewImageTool {
  pub fn get_specification(&self) -> Tool {
    Tool {
      name: "view_image".into(),
      description: "View a local image using its absolute file path. A normalized image (up to 2048 pixels per edge and 2 MiB) is retained in context; the original file is unchanged. Animated images use the first frame. The application sends native image input when supported, otherwise a file notice.".into(),
      input_schema: json!({"type":"object","additionalProperties":false,"required":["path"],"properties":{"path":{"type":"string","description":"Absolute path to a local image file."}}}),
    }
  }
  async fn read_image(&self, args: Arguments) -> Result<ToolOutcome, String> {
    if !args.path.is_absolute() {
      return Err("path must be absolute".into());
    }
    let metadata = tokio::fs::metadata(&args.path).await.map_err(|e| e.to_string())?;
    if !metadata.is_file() {
      return Err("path must refer to a regular file".into());
    }
    const MAX_IMAGE_BYTES: u64 = 100 * 1024 * 1024;
    if metadata.len() > MAX_IMAGE_BYTES {
      return Err("image exceeds 100 MiB; use shell to resize it first".into());
    }
    let prepared = crate::utils::model_image::prepare_file(&args.path).await?;
    let mime_type = prepared.mime;
    Ok(ToolOutcome::SuccessWithInput {
      output: json!({"path":args.path,"mime_type":mime_type,"byte_count":prepared.bytes.len(),"original_byte_count":metadata.len(),"model_image_max_edge":2048}),
      input: vec![
        ContentBlock::Text { text: format!("Image from local file {}", args.path.display()) },
        ContentBlock::Image {
          mime_type: mime_type.into(),
          data_base64: base64::engine::general_purpose::STANDARD.encode(&prepared.bytes),
        },
      ],
    })
  }
}
impl ToolExecutor for ViewImageTool {
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    if control.is_cancelled() {
      return ToolOutcome::Cancelled;
    }
    let parsed =
      super::arguments::normalize(&call.arguments, &self.get_specification().input_schema)
        .and_then(|value| serde_json::from_value(value).map_err(|error| error.to_string()));
    let args = match parsed {
      Ok(args) => args,
      Err(error) => {
        return ToolOutcome::Failed(super::arguments::error(
          "view_image",
          error,
          r#"{"path":"/absolute/path/image.png"}"#,
        ));
      }
    };
    match control.run_until_cancelled(self.read_image(args)).await {
      Some(result) => result.unwrap_or_else(ToolOutcome::Failed),
      None => ToolOutcome::Cancelled,
    }
  }
}
