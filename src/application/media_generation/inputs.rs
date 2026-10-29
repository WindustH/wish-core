//! Resolve session-local assets without exposing the local server to the Internet.
use super::*;
use crate::extension_host::media::video::{ResolvedInput, VideoInput};
use base64::Engine;
use tokio::io::AsyncReadExt;

pub(super) async fn resolve(
  inputs: &[VideoInput],
  directory: &Path,
  protocol: MediaProtocol,
  service: &MediaProvider,
  spec: &config::MediaPreset,
  credentials: &crate::protocol::endpoint::Credentials,
  state: &State,
) -> Result<Vec<ResolvedInput>, String> {
  let mut result = Vec::new();
  let mut remaining = 40 * 1024 * 1024u64;
  for input in inputs {
    let uri = if input.voice_id.is_some() {
      // Preset voices are provider identifiers, not downloadable or uploadable assets.
      String::new()
    } else if let Some(job) = &input.job_id {
      jobs::source_task(directory, job, service).await?
    } else if let Some(url) = &input.url {
      validate_input_url(url, protocol)?;
      url.clone()
    } else {
      let id = input.blob_id.as_deref().ok_or("missing input blob")?;
      if !blobs::is_blob_id(id) {
        return Err("input must be a session blob ID".into());
      }
      let root = tokio::fs::canonicalize(directory).await.map_err(|e| e.to_string())?;
      let path =
        tokio::fs::canonicalize(root.join(id)).await.map_err(|_| "input blob not found")?;
      if path.parent() != Some(root.as_path()) {
        return Err("input blob must belong to this session".into());
      }
      let needs_url = !protocol
        .video_capabilities(&service.model)
        .ok_or("selected extension has no video input capabilities")?
        .blob_inputs
        .iter()
        .any(|kind| kind == input.role.media_type());
      if needs_url {
        let mut file = tokio::fs::File::open(&path).await.map_err(|e| e.to_string())?;
        let mut header = vec![0; 512];
        let n = file.read(&mut header).await.map_err(|e| e.to_string())?;
        header.truncate(n);
        let mime = crate::utils::image_format::supported_mime(&header)
          .or_else(|| media::mime(&header))
          .ok_or("unsupported input media format")?;
        if !mime.starts_with(&format!("{}/", input.role.media_type())) {
          return Err("input blob media type does not match its role".into());
        }
        // Generated output URLs remain private in the journal and are only forwarded to the selected service.
        let url=jobs::source_url(directory,id,service).await?.ok_or("this provider requires a public HTTPS URL for this input; arbitrary local video upload is not supported, use url or a generated video blob with a recorded upstream URL")?;
        validate_input_url(&url, protocol)?;
        url
      } else {
        let file = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
        let limit = remaining.min(match input.role.media_type() {
          "image" => 30_000_000,
          "audio" => 15_000_000,
          _ => 40 * 1024 * 1024,
        });
        if file.metadata().await.map_err(|e| e.to_string())?.len() > limit {
          return Err(
            "video inputs exceed upload limit (40 MiB combined; image 30 MB, audio 15 MB)".into(),
          );
        }
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes).await.map_err(|e| e.to_string())?;
        if bytes.len() as u64 > limit {
          return Err("video inputs exceed upload limit".into());
        }
        remaining -= bytes.len() as u64;
        let mime = crate::utils::image_format::supported_mime(&bytes)
          .or_else(|| media::mime(&bytes))
          .ok_or("unsupported input media format")?;
        let expected = input.role.media_type();
        if !mime.starts_with(&format!("{expected}/")) {
          return Err("input blob media type does not match its role".into());
        }
        if !matches!(
          mime,
          "image/png" | "image/jpeg" | "image/webp" | "video/mp4" | "audio/mpeg" | "audio/wav"
        ) {
          return Err("unsupported input encoding for video generation".into());
        }
        if protocol.0.requires_upload(bytes.len()) {
          protocol
            .0
            .upload(
              bytes,
              mime,
              service.base_url.as_deref().filter(|v| !v.is_empty()).unwrap_or(&spec.base_url),
              credentials,
              &state.transport(service.proxy_enabled, TransferProfile::Upload)?,
              &state.transport(service.proxy_enabled, TransferProfile::Status)?,
            )
            .await?
            .ok_or("extension upload did not return a URI")?
        } else {
          crate::utils::blocking::cpu(move || {
            format!(
              "data:{mime};base64,{}",
              base64::engine::general_purpose::STANDARD.encode(bytes)
            )
          })
          .await
          .map_err(|e| e.to_string())?
        }
      }
    };
    result.push(ResolvedInput {
      role: input.role,
      uri,
      timestamp_s: input.timestamp_s,
      voice_id: input.voice_id.clone(),
    });
  }
  Ok(result)
}
pub(super) fn validate_input_url(value: &str, protocol: MediaProtocol) -> Result<(), String> {
  protocol.0.validate_input_url(value)
}
