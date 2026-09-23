//! Read a local image as native model input, without modifying the source file.
use crate::{
  executor::{
    ExecutionControl,
    tool::{ToolCall, ToolExecutor, ToolOutcome},
  },
  protocol::{ContentBlock, Tool},
};
use base64::Engine;
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;
use tokio::io::AsyncReadExt;

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
      description: "View a local image using its absolute file path. The image is retained in context. The application sends native image input when supported, otherwise a file notice.".into(),
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
    const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
    if metadata.len() > MAX_IMAGE_BYTES {
      return Err("image exceeds 20 MiB; use shell to resize it first".into());
    }
    let file = tokio::fs::File::open(&args.path).await.map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_IMAGE_BYTES + 1).read_to_end(&mut bytes).await.map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
      return Err("image exceeds 20 MiB; use shell to resize it first".into());
    }
    let mime_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
      "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
      "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
      "image/gif"
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
      "image/webp"
    } else {
      return Err("unsupported image format; expected PNG, JPEG, GIF or WebP".into());
    };
    Ok(ToolOutcome::SuccessWithInput {
      output: json!({"path":args.path,"mime_type":mime_type,"byte_count":bytes.len()}),
      input: vec![
        ContentBlock::Text { text: format!("Image from local file {}", args.path.display()) },
        ContentBlock::Image {
          mime_type: mime_type.into(),
          data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
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
    let args = match serde_json::from_value(call.arguments.clone()) {
      Ok(args) => args,
      Err(error) => return ToolOutcome::Failed(error.to_string()),
    };
    tokio::select! {
      biased;
      _ = control.wait_for_cancellation() => ToolOutcome::Cancelled,
      result = self.read_image(args) => result.unwrap_or_else(ToolOutcome::Failed),
    }
  }
}
