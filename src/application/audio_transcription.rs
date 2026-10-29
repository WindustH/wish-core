//! ASR uses the same configured media accounts/order, but a separate input and result contract.
use super::{
  accounts::AccountRegistry,
  media_generation::{MediaHub, State, TransferProfile, presets},
};
use crate::{protocol::attempt::Transport, storage::blobs};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
  pub blob_id: String,
  #[serde(default)]
  pub provider: Option<String>,
  #[serde(default)]
  pub language: Option<String>,
  #[serde(default)]
  pub diarize: bool,
}
impl MediaHub {
  pub async fn transcribe(
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
  if !blobs::is_blob_id(&request.blob_id) {
    return Err("blob_id must identify an audio/video file in this session".into());
  }
  if request
    .language
    .as_ref()
    .is_some_and(|v| !(2..=3).contains(&v.len()) || !v.bytes().all(|c| c.is_ascii_alphabetic()))
  {
    return Err("language must be a 2 or 3 letter ISO code".into());
  }
  let selected = request
    .provider
    .as_deref()
    .or_else(|| {
      config
        .order
        .iter()
        .find(|id| config.providers.get(*id).is_some_and(|p| p.enabled))
        .map(String::as_str)
    })
    .ok_or("no enabled transcription provider")?;
  let service = config
    .providers
    .get(selected)
    .filter(|p| p.enabled)
    .ok_or("transcription provider is unknown or disabled")?;
  let spec =
    presets().iter().find(|p| p.id == service.preset).ok_or("unknown transcription preset")?;
  let credentials = accounts
    .get(&format!("account:{}", service.credential_id))
    .ok_or("transcription account unavailable")?
    .client
    .get_credentials();
  if credentials.api_key.is_empty() {
    return Err("transcription account has no API key".into());
  }
  let path = tokio::fs::canonicalize(directory.join(&request.blob_id))
    .await
    .map_err(|_| "audio blob not found")?;
  let root = tokio::fs::canonicalize(directory).await.map_err(|e| e.to_string())?;
  if path.parent() != Some(root.as_path()) {
    return Err("audio blob must belong to this session".into());
  }
  let file = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
  const LIMIT: u64 = 32 * 1024 * 1024;
  if file.metadata().await.map_err(|e| e.to_string())?.len() > LIMIT {
    return Err("ASR input exceeds the 32 MiB upload limit".into());
  }
  let mut bytes = Vec::new();
  file.take(LIMIT + 1).read_to_end(&mut bytes).await.map_err(|e| e.to_string())?;
  if bytes.len() as u64 > LIMIT {
    return Err("ASR input exceeds the 32 MiB upload limit".into());
  }
  let mime = crate::storage::media::mime(&bytes)
    .ok_or("ASR input must be a supported audio/video container")?;
  let (route, draft) = spec
    .protocol
    .transcription(&service.model, &bytes, mime, request.language.as_deref(), request.diarize)
    .map_err(|e| e.to_string())?;
  drop(bytes);
  let base = service.base_url.as_deref().filter(|s| !s.is_empty()).unwrap_or(&spec.base_url);
  let call = spec.protocol.call(base, &route, credentials, draft).map_err(|e| e.to_string())?;
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
  let output: Value = serde_json::from_slice(&reply.body)
    .map_err(|_| Failure::Unknown("invalid ASR reply".into()))?;
  let text = output["text"]
    .as_str()
    .ok_or_else(|| Failure::Unknown("ASR reply has no transcript".into()))?;
  let artifact = blobs::save_upload(directory, &reply.body)
    .await
    .map_err(|e| Failure::Unknown(e.to_string()))?;
  Ok(
    json!({"provider":selected,"model":service.model,"language":output.get("language_code").or_else(|| output.get("language")),"text":text.chars().take(32_000).collect::<String>(),"truncated":text.chars().count()>32_000,"transcript":artifact,"format":"json","word_timestamps":output["words"].as_array().is_some_and(|a|!a.is_empty())}),
  )
}
