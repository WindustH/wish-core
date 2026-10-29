//! Transcription contracts retain their supplier type until the application adapter.
use super::*;
impl SessionTools {
  pub(super) async fn execute_openai_transcribe(
    &self,
    request: crate::extensions::openai::transcribe_tool::Request,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    self
      .execute_transcription(
        crate::application::audio_transcription::Request::Openai(request),
        control,
      )
      .await
  }
  pub(super) async fn execute_siliconflow_transcribe(
    &self,
    request: crate::extensions::siliconflow::transcribe_tool::Request,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    self
      .execute_transcription(
        crate::application::audio_transcription::Request::Siliconflow(request),
        control,
      )
      .await
  }
  pub(super) async fn execute_elevenlabs_transcribe(
    &self,
    request: crate::extensions::elevenlabs::transcribe_tool::Request,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    self
      .execute_transcription(
        crate::application::audio_transcription::Request::Elevenlabs(request),
        control,
      )
      .await
  }
  pub(super) async fn execute_xai_transcribe(
    &self,
    request: crate::extensions::xai::Transcribe,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    self
      .execute_transcription(
        crate::application::audio_transcription::Request::Xai(request),
        control,
      )
      .await
  }
  async fn execute_transcription(
    &self,
    request: crate::application::audio_transcription::Request,
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let registration = request.registration();
    let Some(owner) = self.slot.upgrade() else {
      return ToolOutcome::Failed("session is closed".into());
    };
    let Some(app) = owner.app.upgrade() else {
      return ToolOutcome::Failed("server is closed".into());
    };
    let accounts = app.get_service_accounts();
    let operation = app.asr.transcribe_scoped(request, &owner.blob_dir, &accounts);
    match control.run_until_cancelled(operation).await {
      Some(Ok(output)) => ToolOutcome::Report {
        text: output.to_string(),
        metadata: output.clone(),
        output,
        failed: false,
      },
      Some(Err(error)) => error.outcome(),
      None => ToolOutcome::Unknown(format!(
        "{} interrupted; do not automatically resubmit",
        registration.name
      )),
    }
  }
}
