//! Shared RunningHub resource-upload wire format for image and video tools.
use crate::protocol::{attempt::Transport, endpoint::Draft, error::Error};
use serde_json::Value;
pub fn upload(bytes: &[u8], mime: &str) -> Result<Draft, Error> {
  let ext = match mime {
    "image/png" => "png",
    "image/jpeg" => "jpg",
    "image/webp" => "webp",
    "video/mp4" => "mp4",
    "audio/mpeg" => "mp3",
    "audio/wav" => "wav",
    _ => return Err(Error::Build("unsupported RunningHub upload format".into())),
  };
  let boundary = format!("wish-{}", uuid::Uuid::new_v4());
  let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"input.{ext}\"\r\nContent-Type: {mime}\r\n\r\n").into_bytes();
  body.extend_from_slice(bytes);
  body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
  Ok(Draft::post(
    Some("/openapi/v2/media/upload/binary".into()),
    vec![("content-type".into(), format!("multipart/form-data; boundary={boundary}"))],
    body,
  ))
}
pub fn uploaded_url(value: &Value) -> Result<String, Error> {
  if value["code"].as_u64() != Some(200) {
    return Err(Error::Build("RunningHub upload failed; generation was not submitted".into()));
  }
  let url = value["data"]["download_url"]
    .as_str()
    .ok_or_else(|| Error::Build("RunningHub upload returned no URL".into()))?;
  let parsed = crate::extensions::shared::urls::validate_url(url)?;
  if parsed.scheme() != "https" || parsed.fragment().is_some() {
    return Err(Error::Build("RunningHub upload must return a public HTTPS URL".into()));
  }
  Ok(url.into())
}

pub async fn send(
  bytes: Vec<u8>,
  mime: &str,
  base: &str,
  credentials: &crate::protocol::endpoint::Credentials,
  upload_transport: &(impl Transport + Sync),
) -> Result<String, String> {
  let mime = mime.to_owned();
  let draft = crate::utils::blocking::cpu(move || upload(&bytes, &mime))
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
  let call = super::endpoint::call(base, "/openapi/v2/media/upload/binary", credentials, draft)
    .map_err(|e| e.to_string())?;
  let response = upload_transport.execute(&call).await.map_err(|e| e.to_string())?;
  let value = crate::protocol::http_error::read_provider_json(response, "runninghub_upload")
    .map_err(|e| e.to_string())?;
  let url = uploaded_url(&value).map_err(|e| e.to_string())?;
  crate::extensions::shared::urls::validate_asset_url(&url, None)?;
  Ok(url)
}
