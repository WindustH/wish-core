//! Media tools share discovery and typed generation requests; application owns durable jobs.
use crate::{
  protocol::Tool,
  session::{ToolCall, ToolOutcome},
  tool::contract::ToolExecutor,
  utils::control::ExecutionControl,
};
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc};
pub type Backend = Arc<
  dyn Fn(Value, String) -> Pin<Box<dyn Future<Output = Result<Value, String>> + Send>>
    + Send
    + Sync,
>;
pub struct MediaGenerateTool {
  pub video: bool,
  pub backend: Backend,
  pub providers: super::providers::Listing,
}
impl MediaGenerateTool {
  pub fn specification(&self) -> Tool {
    let mut tool=super::providers::specification(Tool {
      name:if self.video {"video_generate"}else{"audio_generate"}.into(),
      description:"Generate media into session-local Wish blob files. Requires prompt for run; audio also requires kind (speech, music or sound_effect). Speech prompt is the exact text to read and requires a voice ID in options or service settings. Calls may incur charges. Video run returns a durable job_id: use status with that job_id until succeeded (wait at least poll_after_seconds between calls). Status resumes queries/downloads after restart, never repeats generation. list_jobs recovers this session's jobs after interruption. An outcome_unknown means upstream may have completed it; do not automatically resubmit. Paths appear only after files are stored; a returned path does not mean you have seen or heard the media.".into(),
      input_schema:json!({"type":"object","additionalProperties":false,"properties":{
        "prompt":{"type":"string"},"kind":{"type":"string","enum":["video","speech","music","sound_effect"]},
        "job_id":{"type":"string"},"options":{"type":"object","additionalProperties":false,"properties":{
          "duration":{"type":"number","description":"Seconds. Video H3: integer 4..15 (Max 5..15); music 3..600; sound effects 0.5..30. Omit for speech."},
          "voice":{"type":"string","description":"Speech voice ID; omit to use configured voice."},
          "aspect_ratio":{"type":"string"},"resolution":{"type":"string"},"instrumental":{"type":"boolean"}
        }}
      }})
    });
    tool.input_schema["properties"]["action"]["enum"] =
      json!(["run", "list_providers", "status", "list_jobs"]);
    tool
  }
}
impl ToolExecutor for MediaGenerateTool {
  async fn execute(&self, call: &ToolCall, control: &ExecutionControl) -> ToolOutcome {
    let args = match super::providers::prepare(&call.arguments, &self.specification().input_schema)
    {
      Ok(v) => v,
      Err(e) => return ToolOutcome::Failed(e),
    };
    if let Some(outcome) = super::providers::listing(&args, &self.providers) {
      return outcome;
    }
    let action = args["action"].as_str().unwrap_or("run");
    if action == "list_jobs" && args.as_object().is_some_and(|o| o.len() != 1) {
      return ToolOutcome::Failed("list_jobs accepts only action".into());
    }
    if action == "status"
      && args.as_object().is_some_and(|o| o.keys().any(|k| k != "action" && k != "job_id"))
    {
      return ToolOutcome::Failed(
        "status accepts only action and job_id; it uses the original provider".into(),
      );
    }
    match control.run_until_cancelled((self.backend)(args, call.call_id.clone())).await {
      None => ToolOutcome::Unknown(
        "Media operation interrupted. Use list_jobs/status to recover; do not repeat generation."
          .into(),
      ),
      Some(Err(e)) => ToolOutcome::Failed(e),
      Some(Ok(output)) => ToolOutcome::Report {
        text: output.to_string(),
        metadata: output.clone(),
        output,
        failed: false,
      },
    }
  }
}
