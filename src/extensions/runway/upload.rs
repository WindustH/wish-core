//! Runway ephemeral upload workflow; signed storage uploads never receive API credentials.
use crate::protocol::attempt::{Call, Method, Transport};
use serde_json::json;
pub fn requires_upload(byte_len: usize) -> bool {
  byte_len > 3_000_000
}
pub async fn send(
  bytes: Vec<u8>,
  mime: &str,
  base: &str,
  credentials: &crate::protocol::endpoint::Credentials,
  upload_transport: &(impl Transport + Sync),
  status_transport: &(impl Transport + Sync),
) -> Result<String, String> {
  use crate::protocol::endpoint::Draft;
  let ext = match mime {
    "image/png" => "png",
    "image/jpeg" => "jpg",
    "image/webp" => "webp",
    "video/mp4" => "mp4",
    _ => return Err("unsupported Runway upload format".into()),
  };
  let draft = Draft::post_json(&json!({"type":"ephemeral","filename":format!("input.{ext}")}))
    .map_err(|e| e.to_string())?;
  let call =
    super::endpoint::call(base, "/v1/uploads", credentials, draft).map_err(|e| e.to_string())?;
  let reply = status_transport.execute(&call).await.map_err(|e| e.to_string())?;
  let value = crate::protocol::http_error::read_provider_json(reply, "runway_upload")
    .map_err(|e| e.to_string())?;
  let url = value["uploadUrl"].as_str().ok_or("upload reply has no URL")?;
  let target = crate::extensions::shared::urls::validate_url(url).map_err(|e| e.to_string())?;
  let origin = crate::extensions::shared::urls::validate_url(base).map_err(|e| e.to_string())?;
  if target.scheme() != "https"
    && !(origin.scheme() == "http" && target.origin() == origin.origin())
  {
    return Err("upload destination must use HTTPS".into());
  }
  let uri = value["runwayUri"]
    .as_str()
    .filter(|v| v.starts_with("runway://"))
    .ok_or("upload reply has no runway URI")?;
  let fields = value["fields"].as_object().ok_or("upload reply has no fields")?;
  let fields: Vec<(String, String)> = fields
    .iter()
    .map(|(name, value)| {
      if !name.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)) {
        return Err("invalid upload field name".to_owned());
      }
      Ok((name.clone(), value.as_str().ok_or("invalid upload field value")?.to_owned()))
    })
    .collect::<Result<_, String>>()?;
  let mime = mime.to_owned();
  let draft = crate::utils::blocking::cpu(move || {
    let fields: Vec<_> =
      fields.iter().map(|(name, value)| (name.as_str(), value.clone())).collect();
    crate::extensions::shared::multipart::file(&fields, &bytes, &mime, ext)
  })
  .await
  .map_err(|e| e.to_string())?;
  // Signed upload fields authorize this destination; never forward the API bearer token.
  let reply = upload_transport
    .execute(&Call {
      method: Method::Post,
      url: target.into(),
      headers: draft.headers,
      body: draft.body,
    })
    .await
    .map_err(|e| e.to_string())?;
  if !reply.is_success() {
    return Err(format!(
      "media upload failed (HTTP {}); generation was not submitted",
      reply.status
    ));
  }
  Ok(uri.into())
}
