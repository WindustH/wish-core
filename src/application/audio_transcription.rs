//! ASR uses the same configured media accounts/order, but a separate input and result contract.
use super::{
  accounts::AccountRegistry,
  config::ProxyConfig,
  media_generation::{MediaConfig, MediaHub, presets},
};
use crate::{
  protocol::{
    attempt::{Limits, Transport},
    endpoint::{AuthScheme, Draft, Endpoint},
  },
  storage::blobs,
  transport::ReqwestTransport,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
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
  ) -> Result<Value, String> {
    let _permit = self.permits.acquire().await.map_err(|e| e.to_string())?;
    let state = self.state.read().unwrap().clone();
    execute(&state.config, &state.proxy, request, directory, accounts).await
  }
}
async fn execute(
  config: &MediaConfig,
  proxy: &ProxyConfig,
  request: Request,
  directory: &Path,
  accounts: &AccountRegistry,
) -> Result<Value, String> {
  use tokio::io::AsyncReadExt;
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
  let boundary = format!("wish-{}", uuid::Uuid::new_v4());
  let mut body = Vec::new();
  let mut fields = vec![
    ("model_id", service.model.clone()),
    ("diarize", request.diarize.to_string()),
    ("timestamps_granularity", "word".into()),
  ];
  if let Some(language) = request.language {
    fields.push(("language_code", language));
  }
  for (name, value) in fields {
    body.extend_from_slice(
      format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
        .as_bytes(),
    );
  }
  body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"input\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
  body.extend_from_slice(&bytes);
  drop(bytes);
  body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
  let draft = Draft::post(
    None,
    vec![("content-type".into(), format!("multipart/form-data; boundary={boundary}"))],
    body,
  );
  let base = service.base_url.as_deref().filter(|s| !s.is_empty()).unwrap_or(&spec.base_url);
  let call = Endpoint::new(base, "/v1/speech-to-text", AuthScheme::Header("xi-api-key"))
    .map_err(|e| e.to_string())?
    .build_call(draft, credentials, crate::utils::time::unix_seconds())
    .map_err(|e| e.to_string())?;
  let transport = ReqwestTransport::new(
    Limits {
      first_byte: Duration::from_secs(300),
      total: Duration::from_secs(600),
      max_response_bytes: 16 * 1024 * 1024,
      ..Default::default()
    },
    proxy.policy(service.proxy_enabled),
  )
  .map_err(|e| e.to_string())?;
  let reply=transport.execute(&call).await.map_err(|_|"ASR connection failed; upstream may have processed the file. Do not automatically repeat the paid request.")?;
  if !reply.is_success() {
    return Err(format!("ASR HTTP {}; no other provider was tried", reply.status));
  }
  let output: Value = serde_json::from_slice(&reply.body).map_err(|_| "invalid ASR reply")?;
  let text = output["text"].as_str().ok_or("ASR reply has no transcript")?;
  let artifact = blobs::save_upload(directory, &reply.body).await.map_err(|e| e.to_string())?;
  Ok(
    json!({"provider":selected,"model":service.model,"language":output["language_code"],"text":text.chars().take(32_000).collect::<String>(),"truncated":text.chars().count()>32_000,"transcript":artifact,"format":"json","word_timestamps":output["words"].as_array().is_some_and(|a|!a.is_empty())}),
  )
}
