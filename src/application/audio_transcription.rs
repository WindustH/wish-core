//! ASR uses the same configured media accounts/order, but a separate input and result contract.
use super::{
  accounts::AccountRegistry,
  media_generation::{MediaHub, State, TransferProfile, presets},
};
pub use crate::extension_host::media::transcription::Request;
use crate::{protocol::attempt::Transport, storage::blobs};
use serde_json::Value;
use std::path::Path;
impl MediaHub {
  pub async fn transcribe_scoped(
    &self,
    request: Request,
    directory: &Path,
    accounts: &AccountRegistry,
  ) -> Result<Value, crate::tool::paid_operation::Failure> {
    let _permit = self.permits.acquire().await.map_err(|e| e.to_string())?;
    let state = self.state.read().unwrap().clone();
    execute(&state, request, directory, accounts).await
  }
}
async fn execute(
  state: &State,
  request: Request,
  directory: &Path,
  accounts: &AccountRegistry,
) -> Result<Value, crate::tool::paid_operation::Failure> {
  use tokio::io::AsyncReadExt;
  let config = &state.config;
  let allowed_presets = request.registration().service_presets;
  let allowed = |preset: &str| allowed_presets.contains(&preset);
  if !blobs::is_blob_id(request.blob_id()) {
    return Err("blob_id must identify an audio/video file in this session".into());
  }
  let path = tokio::fs::canonicalize(directory.join(request.blob_id()))
    .await
    .map_err(|_| "audio blob not found")?;
  let root = tokio::fs::canonicalize(directory).await.map_err(|e| e.to_string())?;
  if path.parent() != Some(root.as_path()) {
    return Err("audio blob must belong to this session".into());
  }
  let mut file = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
  const LIMIT: u64 = 32 * 1024 * 1024;
  let byte_len = file.metadata().await.map_err(|e| e.to_string())?.len();
  if byte_len > LIMIT {
    return Err("ASR input exceeds the 32 MiB upload limit".into());
  }
  // Read only a bounded signature before routing; don't allocate every candidate's multipart body.
  let mut bytes = Vec::new();
  (&mut file).take(512).read_to_end(&mut bytes).await.map_err(|e| e.to_string())?;
  let mime = crate::storage::media::mime(&bytes)
    .ok_or("ASR input must be a supported audio/video container")?;
  let selected = request
    .service()
    .or_else(|| {
      config
        .order
        .iter()
        .find(|id| {
          config.providers.get(*id).is_some_and(|p| {
            p.enabled
              && allowed(&p.preset)
              && presets().iter().find(|s| s.id == p.preset).is_some_and(|_| {
                request.validate_audio(byte_len, mime).is_ok()
                  && accounts
                    .get(&format!("account:{}", p.credential_id))
                    .is_some_and(|a| request.credentials_ready(a.client.get_credentials()))
              })
          })
        })
        .map(String::as_str)
    })
    .ok_or("no available transcription provider supports these inputs and options")?
    .to_owned();
  let service = config
    .providers
    .get(&selected)
    .filter(|p| p.enabled && allowed(&p.preset))
    .ok_or("transcription provider is unknown or disabled")?;
  let spec =
    presets().iter().find(|p| p.id == service.preset).ok_or("unknown transcription preset")?;
  let credentials = accounts
    .get(&format!("account:{}", service.credential_id))
    .ok_or("transcription account unavailable")?
    .client
    .get_credentials();
  if !request.credentials_ready(credentials) {
    return Err("transcription account has no API key".into());
  }
  request.validate_audio(byte_len, mime).map_err(|e| e.to_string())?;
  file
    .take(LIMIT + 1 - bytes.len() as u64)
    .read_to_end(&mut bytes)
    .await
    .map_err(|e| e.to_string())?;
  if bytes.len() as u64 > LIMIT {
    return Err("ASR input exceeds the 32 MiB upload limit".into());
  }
  let model = service.model.clone();
  let (request, submission) = crate::utils::blocking::cpu(move || {
    let submission = request.submission(&model, &bytes, mime);
    (request, submission)
  })
  .await
  .map_err(|e| e.to_string())?;
  let (route, draft) = submission.map_err(|e| e.to_string())?;
  let base = service.base_url.as_deref().filter(|s| !s.is_empty()).unwrap_or(&spec.base_url);
  let call = request.call(base, &route, credentials, draft).map_err(|e| e.to_string())?;
  let transport = state.transport(service.proxy_enabled, TransferProfile::Transcription)?;
  use crate::tool::paid_operation::Failure;
  let reply =
    transport.execute(&call).await.map_err(|_| Failure::Unknown("ASR connection failed".into()))?;
  if !reply.is_success() {
    let message = format!("ASR HTTP {}; no other provider was tried", reply.status);
    return Err(if reply.status >= 500 {
      Failure::Unknown(message)
    } else {
      Failure::Rejected(message)
    });
  }
  let (body, summary) = crate::utils::blocking::cpu(move || {
    let body = reply.body;
    let summary = request.result(&body);
    (body, summary)
  })
  .await
  .map_err(|e| Failure::Unknown(e.to_string()))?;
  let mut output = summary.map_err(Failure::Unknown)?;
  let artifact =
    blobs::save_upload(directory, &body).await.map_err(|e| Failure::Unknown(e.to_string()))?;
  output["provider"] = selected.into();
  output["model"] = service.model.clone().into();
  output["transcript"] =
    serde_json::to_value(artifact).map_err(|e| Failure::Unknown(e.to_string()))?;
  Ok(output)
}
