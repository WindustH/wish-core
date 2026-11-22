//! Short-lived, single-blob read grants for native media elements (which cannot set auth headers).
use super::*;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
const TTL: u64 = 3600;
fn signature(key: &str, path: &str, expires: u64, version: Option<&str>) -> Hmac<Sha256> {
  let mut mac =
    Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts arbitrary key length");
  mac.update(format!("{path}\n{expires}").as_bytes());
  if let Some(version) = version {
    mac.update(format!("\n{version}").as_bytes());
  }
  mac
}
pub async fn grant(
  State(app): State<Arc<App>>,
  axum::extract::Path((owner, blob)): axum::extract::Path<(String, String)>,
) -> Result<Json<Value>, crate::server::error::ApiError> {
  let source = crate::application::attachments::open(&app, &owner, &blob).await?;
  // Native media elements can reuse decoded data for an identical URL even after
  // remounting. A versioned grant gives changed local files a fresh resource URL.
  let version = hex::encode(&Sha256::digest(source.version.as_bytes())[..16]);
  let path = format!("/sessions/{owner}/blobs/{blob}");
  let expires = crate::utils::time::unix_seconds() + TTL;
  let path = if let Some(key) = &app.bearer_token {
    format!(
      "{path}?expires={expires}&v={version}&signature={}",
      hex::encode(signature(key, &path, expires, Some(&version)).finalize().into_bytes())
    )
  } else {
    format!("{path}?v={version}")
  };
  Ok(Json(json!({"path":path,"expires_at":expires})))
}
pub(super) fn package_url(app: &App, path: &str, expires: u64) -> String {
  if let Some(key) = &app.bearer_token {
    format!(
      "/api{path}?expires={expires}&signature={}",
      hex::encode(signature(key, path, expires, None).finalize().into_bytes())
    )
  } else {
    format!("/api{path}")
  }
}
pub fn authorized(request: &Request, key: &str) -> bool {
  if !matches!(*request.method(), Method::GET | Method::HEAD) {
    return false;
  }
  let path = request.uri().path().strip_prefix("/api").unwrap_or(request.uri().path());
  let parts: Vec<_> = path.split('/').collect();
  let blob = matches!(parts.as_slice(),["","sessions",owner,"blobs",id] if uuid::Uuid::parse_str(owner).is_ok() && crate::storage::blobs::is_blob_id(id));
  let export = matches!(parts.as_slice(),["","exports",id] if uuid::Uuid::parse_str(id).is_ok());
  let archive =
    matches!(parts.as_slice(),["","archives",id,"download"] if uuid::Uuid::parse_str(id).is_ok());
  if !blob && !export && !archive {
    return false;
  }
  let Some(query) = request.uri().query().filter(|q| q.len() < 200) else {
    return false;
  };
  let mut expires = None;
  let mut signed = None;
  let mut version = None;
  for field in query.split('&') {
    match field.split_once('=') {
      Some(("expires", value)) if expires.is_none() => expires = value.parse::<u64>().ok(),
      Some(("signature", value)) if signed.is_none() => signed = hex::decode(value).ok(),
      Some(("v", value))
        if version.is_none()
          && value.len() == 32
          && value.bytes().all(|b| b.is_ascii_hexdigit()) =>
      {
        version = Some(value)
      }
      _ => return false,
    }
  }
  let (Some(expires), Some(signed)) = (expires, signed) else {
    return false;
  };
  let now = crate::utils::time::unix_seconds();
  expires >= now
    && expires <= now.saturating_add(TTL)
    && signature(key, path, expires, version).verify_slice(&signed).is_ok()
}
