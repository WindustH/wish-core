//! Bounded credential-free artifact redirects; storage remains with the caller.
use crate::{
  protocol::attempt::{Call, Method, Transport},
  transport::ReqwestTransport,
};
pub(crate) async fn download(
  transport: &ReqwestTransport,
  url: &str,
  user_agent: &str,
  trusted_base: &str,
) -> Result<Vec<u8>, String> {
  let mut url = super::artifact_destinations::validate(url, trusted_base)?;
  let public =
    transport.with_resolver(super::artifact_destinations::PublicDns).map_err(|e| e.to_string())?;
  // CDN requests and redirects never receive account credentials.
  for _ in 0..5 {
    let selected =
      if super::artifact_destinations::trusted(&url, trusted_base) { transport } else { &public };
    let reply = selected
      .execute(&Call {
        method: Method::Get,
        url: url.to_string(),
        headers: vec![("user-agent".into(), user_agent.into())],
        body: Vec::new(),
      })
      .await
      .map_err(|e| e.to_string())?;
    if reply.is_success() {
      return Ok(reply.body);
    }
    if matches!(reply.status, 301 | 302 | 303 | 307 | 308) {
      let next = url
        .join(reply.get_header("location").ok_or("image redirect has no location")?)
        .map_err(|_| "invalid image redirect")?;
      url = super::artifact_destinations::validate(next.as_str(), trusted_base)?;
    } else {
      return Err(format!("generated image download failed (HTTP {})", reply.status));
    }
  }
  Err("too many generated image redirects".into())
}

/// Open a generated artifact as a stream, following only bounded credential-free redirects.
/// The caller retains ownership of format validation, byte limits and publishing the blob.
pub(crate) async fn open_stream(
  transport: &ReqwestTransport,
  url: &str,
  trusted_base: &str,
) -> Result<<ReqwestTransport as Transport>::Stream, String> {
  use crate::protocol::attempt::ReplyStream;
  let mut target = super::artifact_destinations::validate(url, trusted_base)?;
  let public =
    transport.with_resolver(super::artifact_destinations::PublicDns).map_err(|e| e.to_string())?;
  for _ in 0..5 {
    let selected = if super::artifact_destinations::trusted(&target, trusted_base) {
      transport
    } else {
      &public
    };
    let stream = selected
      .execute_stream(&Call {
        method: Method::Get,
        url: target.to_string(),
        headers: Vec::new(),
        body: Vec::new(),
      })
      .await
      .map_err(|_| "artifact download connection failed".to_owned())?;
    if !matches!(stream.get_status(), 301 | 302 | 303 | 307 | 308) {
      return Ok(stream);
    }
    let location = crate::protocol::attempt::find_header(stream.get_headers(), "location")
      .ok_or("artifact redirect has no location")?;
    let next = target.join(location).map_err(|_| "invalid artifact redirect")?;
    target = super::artifact_destinations::validate(next.as_str(), trusted_base)?;
  }
  Err("too many artifact redirects".into())
}
