//! Session-owned blob access and durable job scope for Runway's native contract.
use super::*;
use crate::extensions::runway::{Request, operation::Operation};
impl SessionTools {
  pub(super) async fn execute_runway_video(
    &self,
    request: Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let operation = match request.prepare() {
      Ok(operation) => operation,
      Err(error) => return ToolOutcome::Failed(error),
    };
    let request = match operation {
      Operation::Submit(submission) => {
        let options = match serde_json::to_value(submission.options) {
          Ok(Value::Object(options)) => options,
          Ok(_) => return ToolOutcome::Failed("invalid Runway options".into()),
          Err(error) => return ToolOutcome::Failed(error.to_string()),
        };
        crate::application::media_generation::Request {
          action: Some("run".into()),
          provider: submission.service,
          prompt: submission.prompt,
          mode: Some(submission.mode),
          inputs: submission.inputs,
          options,
          runway: Some(submission.plan),
          ..Default::default()
        }
      }
      Operation::Status(job_id) => crate::application::media_generation::Request {
        action: Some("status".into()),
        job_id: Some(job_id),
        ..Default::default()
      },
      Operation::ListJobs => crate::application::media_generation::Request {
        action: Some("list_jobs".into()),
        ..Default::default()
      },
    };
    let Some(owner) = self.slot.upgrade() else {
      return ToolOutcome::Failed("session is closed".into());
    };
    let Some(app) = owner.app.upgrade() else {
      return ToolOutcome::Failed("server is closed".into());
    };
    let accounts = app.get_service_accounts();
    let operation = app.videos.execute_scoped(
      request,
      &call.call_id,
      &owner.blob_dir,
      &accounts,
      crate::application::media_generation::Scope {
        video: true,
        presets: crate::extensions::runway::REGISTRATION.service_presets,
      },
    );
    match control.run_until_cancelled(operation).await {
      Some(Ok(output)) => ToolOutcome::Report {
        text: output.to_string(),
        metadata: output.clone(),
        output,
        failed: false,
      },
      Some(Err(error)) => ToolOutcome::Failed(error),
      None => ToolOutcome::Unknown(
        "Runway video interrupted; use runway_video list_jobs/status, do not repeat generation."
          .into(),
      ),
    }
  }
}
