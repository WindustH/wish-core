use crate::{
  protocol::Tool,
  session::{ToolCall, ToolOutcome},
  tool::contract::ToolExecutor,
  utils::control::ExecutionControl,
};
use serde_json::json;
pub type Backend = std::sync::Arc<
  dyn Fn(
      serde_json::Value,
      String,
    ) -> std::pin::Pin<
      Box<
        dyn std::future::Future<Output = Result<serde_json::Value, super::paid_operation::Failure>>
          + Send,
      >,
    > + Send
    + Sync,
>;
pub struct AudioTranscribeTool {
  pub backend: Backend,
  pub providers: super::providers::Listing,
}
impl AudioTranscribeTool {
  pub fn specification(&self) -> Tool {
    super::providers::specification(Tool{name:"audio_transcribe".into(),description:"Transcribe an audio/video file already stored in this session's Wish blobs. Supply its blob_id (the 64-character basename of its local blob path). Supports MP3/WAV/FLAC/Ogg/MP4/WebM, up to 32 MiB. Returns transcript text, detected language and the local path to full transcript JSON including word timestamps/speaker IDs when available. Optional language is an ISO 2/3 letter code; diarize requests speaker attribution. ASR may incur charges; interrupted requests are not automatically repeated or sent to another provider.".into(),input_schema:json!({"type":"object","additionalProperties":false,"properties":{"blob_id":{"type":"string"},"language":{"type":"string"},"diarize":{"type":"boolean"}}})})
  }
}
impl ToolExecutor for AudioTranscribeTool {
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let args = match super::providers::prepare(&call.arguments, &self.specification().input_schema)
    {
      Ok(v) => v,
      Err(e) => return ToolOutcome::Failed(e),
    };
    if let Some(outcome) = super::providers::listing(&args, &self.providers) {
      return outcome;
    }
    match control.run_until_cancelled((self.backend)(args, call.call_id.clone())).await {
      None => ToolOutcome::Unknown(
        "ASR interrupted; upstream may have processed the file. Do not automatically repeat it."
          .into(),
      ),
      Some(Err(e)) => e.outcome(),
      Some(Ok(output)) => ToolOutcome::Report {
        text: output.to_string(),
        metadata: output.clone(),
        output,
        failed: false,
      },
    }
  }
}
