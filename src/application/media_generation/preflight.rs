//! Pure submission preflight shared by ordered routing and explicit provider calls.
//! No uploads, credential refreshes or paid requests may run while choosing a provider.
use super::{MediaKind, MediaProtocol, MediaProvider, Parameters, Request};
use crate::extension_host::media::video::{ResolvedInput, VideoMode};
use crate::protocol::endpoint::Draft;

pub(super) fn prepare(
  request: &Request,
  service: &MediaProvider,
  protocol: MediaProtocol,
  kind: MediaKind,
  mode: VideoMode,
) -> Result<(Parameters, String, Draft), String> {
  let options = protocol
    .parameters(&service.parameters, &request.options, mode, &request.inputs)
    .map_err(|e| e.to_string())?;
  if let Some(plan) = &request.plan {
    if protocol.0.id() != plan.driver_id() {
      return Err("native plan requires its own extension service".into());
    }
    // Validate every source before any large earlier asset can be uploaded.
    for input in &request.inputs {
      if let Some(url) = &input.url {
        super::inputs::validate_input_url(url, protocol)?;
      }
      if input.blob_id.as_deref().is_some_and(|id| !crate::storage::blobs::is_blob_id(id)) {
        return Err("input must be a session blob ID".into());
      }
    }
    let placeholders: Vec<_> = request
      .inputs
      .iter()
      .map(|input| ResolvedInput {
        role: input.role,
        uri: "https://placeholder.invalid/input".into(),
        timestamp_s: None,
        voice_id: None,
      })
      .collect();
    let (route, draft) = plan
      .build(&crate::extensions::media_runtime::PlanContext {
        model: &service.model,
        prompt: request.prompt.as_deref().unwrap_or(""),
        defaults: &service.parameters,
        parameters: &options,
        inputs: &placeholders,
      })
      .map_err(|e| e.to_string())?;
    return Ok((options, route, draft));
  }
  if kind == MediaKind::Video {
    protocol
      .validate_video_inputs(&service.model, mode, &request.inputs)
      .map_err(|e| e.to_string())?;
  }
  let prompt = request.prompt.as_deref().unwrap_or("");
  // Preflight every mode/option before any upload or paid submission.
  let placeholders: Vec<_> = request
    .inputs
    .iter()
    .map(|i| ResolvedInput {
      role: i.role,
      uri: "https://placeholder.invalid/input".into(),
      timestamp_s: i.timestamp_s,
      voice_id: i.voice_id.clone(),
    })
    .collect();
  let (route, draft) = if kind == MediaKind::Video {
    protocol.video_submission(
      &service.model,
      prompt,
      mode,
      &options,
      &placeholders,
      request.modify_mode.as_deref(),
    )
  } else {
    protocol.submission(&service.model, prompt, &options)
  }
  .map_err(|e| e.to_string())?;
  Ok((options, route, draft))
}
