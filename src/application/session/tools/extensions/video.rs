//! Host-owned routing and blob/job access for independently typed video tools.
use super::*;
use crate::extensions::xai::{video::Request, video_operation::Operation};
impl SessionTools {
  pub(super) async fn execute_xai_video(
    &self,
    request: Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let request = match request.prepare() {
      Operation::Submit(submission) => {
        let options = match serde_json::to_value(submission.options) {
          Ok(Value::Object(options)) => options,
          Ok(_) => return ToolOutcome::Failed("invalid xAI video options".into()),
          Err(error) => return ToolOutcome::Failed(error.to_string()),
        };
        crate::application::media_generation::Request {
          action: Some("run".into()),
          prompt: Some(submission.prompt),
          provider: submission.service,
          mode: Some(submission.mode),
          inputs: submission.inputs,
          options,
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
    self
      .execute_video_job(request, &crate::extensions::xai::video::REGISTRATION, call, control)
      .await
  }
}

impl SessionTools {
  pub(super) async fn execute_seedance_video(
    &self,
    request: crate::extensions::seedance::video_tool::Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let options = match serde_json::to_value(request.parameters) {
      Ok(Value::Object(options)) => options,
      Ok(_) => return ToolOutcome::Failed("invalid extension parameter object".into()),
      Err(error) => return ToolOutcome::Failed(error.to_string()),
    };
    let request = crate::application::media_generation::Request {
      action: Some(request.action),
      provider: request.service,
      prompt: request.prompt,
      job_id: request.job_id,
      options,
      mode: request.mode,
      inputs: request.inputs,
      modify_mode: None,
      ..Default::default()
    };
    self
      .execute_video_job(
        request,
        &crate::extensions::seedance::video_tool::REGISTRATION,
        call,
        control,
      )
      .await
  }
  pub(super) async fn execute_minimax_video(
    &self,
    request: crate::extensions::minimax::video_tool::Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let options = match serde_json::to_value(request.parameters) {
      Ok(Value::Object(options)) => options,
      Ok(_) => return ToolOutcome::Failed("invalid extension parameter object".into()),
      Err(error) => return ToolOutcome::Failed(error.to_string()),
    };
    let request = crate::application::media_generation::Request {
      action: Some(request.action),
      provider: request.service,
      prompt: request.prompt,
      job_id: request.job_id,
      options,
      mode: request.mode,
      inputs: request.inputs,
      modify_mode: None,
      ..Default::default()
    };
    self
      .execute_video_job(
        request,
        &crate::extensions::minimax::video_tool::REGISTRATION,
        call,
        control,
      )
      .await
  }
  pub(super) async fn execute_luma_video(
    &self,
    request: crate::extensions::luma::video_tool::Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let options = match serde_json::to_value(request.parameters) {
      Ok(Value::Object(options)) => options,
      Ok(_) => return ToolOutcome::Failed("invalid extension parameter object".into()),
      Err(error) => return ToolOutcome::Failed(error.to_string()),
    };
    let request = crate::application::media_generation::Request {
      action: Some(request.action),
      provider: request.service,
      prompt: request.prompt,
      job_id: request.job_id,
      options,
      mode: request.mode,
      inputs: request.inputs,
      modify_mode: request.modify_mode,
      ..Default::default()
    };
    self
      .execute_video_job(request, &crate::extensions::luma::video_tool::REGISTRATION, call, control)
      .await
  }
  pub(super) async fn execute_runninghub_video(
    &self,
    request: crate::extensions::runninghub::video_tool::Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let options = match serde_json::to_value(request.parameters) {
      Ok(Value::Object(options)) => options,
      Ok(_) => return ToolOutcome::Failed("invalid extension parameter object".into()),
      Err(error) => return ToolOutcome::Failed(error.to_string()),
    };
    let request = crate::application::media_generation::Request {
      action: Some(request.action),
      provider: request.service,
      prompt: request.prompt,
      job_id: request.job_id,
      options,
      mode: request.mode,
      inputs: request.inputs,
      modify_mode: None,
      ..Default::default()
    };
    self
      .execute_video_job(
        request,
        &crate::extensions::runninghub::video_tool::REGISTRATION,
        call,
        control,
      )
      .await
  }
  pub(super) async fn execute_xiaoyunque_video(
    &self,
    request: crate::extensions::xiaoyunque::video_tool::Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let options = match serde_json::to_value(request.parameters) {
      Ok(Value::Object(options)) => options,
      Ok(_) => return ToolOutcome::Failed("invalid extension parameter object".into()),
      Err(error) => return ToolOutcome::Failed(error.to_string()),
    };
    let request = crate::application::media_generation::Request {
      action: Some(request.action),
      provider: request.service,
      prompt: request.prompt,
      job_id: request.job_id,
      options,
      mode: request.mode,
      inputs: request.inputs,
      modify_mode: None,
      ..Default::default()
    };
    self
      .execute_video_job(
        request,
        &crate::extensions::xiaoyunque::video_tool::REGISTRATION,
        call,
        control,
      )
      .await
  }
  async fn execute_video_job(
    &self,
    request: crate::application::media_generation::Request,
    registration: &'static Registration,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
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
        presets: registration.service_presets,
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
      None => ToolOutcome::Unknown(format!(
        "{} interrupted; use status/list_jobs instead of repeating generation",
        registration.name
      )),
    }
  }
}
