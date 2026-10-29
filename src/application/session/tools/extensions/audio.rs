//! Session resources for independently typed audio operations.
use super::*;
use crate::application::media_generation::Request as MediaRequest;
use crate::extension_host::media::MediaKind;
fn parameters(value: impl serde::Serialize) -> Result<serde_json::Map<String, Value>, String> {
  match serde_json::to_value(value) {
    Ok(Value::Object(values)) => Ok(values),
    Ok(_) => Err("expected extension parameter object".into()),
    Err(error) => Err(error.to_string()),
  }
}
impl SessionTools {
  pub(super) async fn execute_minimax_speech(
    &self,
    request: crate::extensions::minimax::Speech,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    use crate::extensions::minimax::Speech;
    let request = match request {
      Speech::Run { text, service, voice, parameters } => MediaRequest {
        action: Some("run".into()),
        kind: Some(MediaKind::Speech),
        provider: service,
        minimax: Some(Box::new(crate::extensions::minimax::speech::Plan {
          text,
          voice,
          parameters,
        })),
        ..Default::default()
      },
      Speech::Status { job_id } => {
        MediaRequest { action: Some("status".into()), job_id: Some(job_id), ..Default::default() }
      }
      Speech::ListJobs {} => {
        MediaRequest { action: Some("list_jobs".into()), ..Default::default() }
      }
    };
    self.execute_audio_job(request, call, control, &crate::extensions::minimax::SPEECH).await
  }
  async fn execute_audio_job(
    &self,
    request: MediaRequest,
    call: &ToolCall,
    control: &ExecutionControl,
    registration: &'static Registration,
  ) -> ToolOutcome {
    let Some(owner) = self.slot.upgrade() else {
      return ToolOutcome::Failed("session is closed".into());
    };
    let Some(app) = owner.app.upgrade() else {
      return ToolOutcome::Failed("server is closed".into());
    };
    let accounts = app.get_service_accounts();
    let operation = app.audio.execute_scoped(
      request,
      &call.call_id,
      &owner.blob_dir,
      &accounts,
      crate::application::media_generation::Scope {
        video: false,
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
        "{} interrupted. Use {} list_jobs/status; do not repeat generation.",
        registration.name, registration.name
      )),
    }
  }
}
impl SessionTools {
  pub(super) async fn execute_xai_speech(
    &self,
    request: crate::extensions::xai::speech::Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    use crate::extensions::xai::speech::Request;
    let request = match request {
      Request::Run(args) => {
        let options = args.parameters();
        MediaRequest {
          action: Some("run".into()),
          kind: Some(MediaKind::Speech),
          prompt: Some(args.text),
          provider: args.service,
          options,
          ..Default::default()
        }
      }
      Request::Status { job_id } => {
        MediaRequest { action: Some("status".into()), job_id: Some(job_id), ..Default::default() }
      }
      Request::ListJobs {} => {
        MediaRequest { action: Some("list_jobs".into()), ..Default::default() }
      }
    };
    self
      .execute_audio_job(request, call, control, &crate::extensions::xai::speech::REGISTRATION)
      .await
  }
  pub(super) async fn execute_openai_speech(
    &self,
    request: crate::extensions::openai::speech_tool::Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    use crate::extensions::openai::speech_tool::Request;
    let request = match request {
      Request::Run { text, service, parameters: options } => {
        let options = match parameters(options) {
          Ok(options) => options,
          Err(error) => return ToolOutcome::Failed(error),
        };
        MediaRequest {
          action: Some("run".into()),
          kind: Some(MediaKind::Speech),
          prompt: Some(text),
          provider: service,
          options,
          ..Default::default()
        }
      }
      Request::Status { job_id } => {
        MediaRequest { action: Some("status".into()), job_id: Some(job_id), ..Default::default() }
      }
      Request::ListJobs {} => {
        MediaRequest { action: Some("list_jobs".into()), ..Default::default() }
      }
    };
    self
      .execute_audio_job(
        request,
        call,
        control,
        &crate::extensions::openai::speech_tool::REGISTRATION,
      )
      .await
  }
  pub(super) async fn execute_elevenlabs_speech(
    &self,
    request: crate::extensions::elevenlabs::speech_tool::Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    use crate::extensions::elevenlabs::speech_tool::Request;
    let request = match request {
      Request::Run { text, service, parameters: options } => {
        let options = match parameters(options) {
          Ok(options) => options,
          Err(error) => return ToolOutcome::Failed(error),
        };
        MediaRequest {
          action: Some("run".into()),
          kind: Some(MediaKind::Speech),
          prompt: Some(text),
          provider: service,
          options,
          ..Default::default()
        }
      }
      Request::Status { job_id } => {
        MediaRequest { action: Some("status".into()), job_id: Some(job_id), ..Default::default() }
      }
      Request::ListJobs {} => {
        MediaRequest { action: Some("list_jobs".into()), ..Default::default() }
      }
    };
    self
      .execute_audio_job(
        request,
        call,
        control,
        &crate::extensions::elevenlabs::speech_tool::REGISTRATION,
      )
      .await
  }
  pub(super) async fn execute_elevenlabs_music(
    &self,
    request: crate::extensions::elevenlabs::music_tool::Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    use crate::extensions::elevenlabs::music_tool::Request;
    let request = match request {
      Request::Run { prompt, service, parameters: options } => {
        let options = match parameters(options) {
          Ok(options) => options,
          Err(error) => return ToolOutcome::Failed(error),
        };
        MediaRequest {
          action: Some("run".into()),
          kind: Some(MediaKind::Music),
          prompt: Some(prompt),
          provider: service,
          options,
          ..Default::default()
        }
      }
      Request::Status { job_id } => {
        MediaRequest { action: Some("status".into()), job_id: Some(job_id), ..Default::default() }
      }
      Request::ListJobs {} => {
        MediaRequest { action: Some("list_jobs".into()), ..Default::default() }
      }
    };
    self
      .execute_audio_job(
        request,
        call,
        control,
        &crate::extensions::elevenlabs::music_tool::REGISTRATION,
      )
      .await
  }
  pub(super) async fn execute_elevenlabs_sound(
    &self,
    request: crate::extensions::elevenlabs::sound_tool::Request,
    call: &ToolCall,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    use crate::extensions::elevenlabs::sound_tool::Request;
    let request = match request {
      Request::Run { prompt, service, parameters: options } => {
        let options = match parameters(options) {
          Ok(options) => options,
          Err(error) => return ToolOutcome::Failed(error),
        };
        MediaRequest {
          action: Some("run".into()),
          kind: Some(MediaKind::SoundEffect),
          prompt: Some(prompt),
          provider: service,
          options,
          ..Default::default()
        }
      }
      Request::Status { job_id } => {
        MediaRequest { action: Some("status".into()), job_id: Some(job_id), ..Default::default() }
      }
      Request::ListJobs {} => {
        MediaRequest { action: Some("list_jobs".into()), ..Default::default() }
      }
    };
    self
      .execute_audio_job(
        request,
        call,
        control,
        &crate::extensions::elevenlabs::sound_tool::REGISTRATION,
      )
      .await
  }
}
