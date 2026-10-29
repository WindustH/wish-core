//! Volcengine OpenAPI HMAC signing (distinct from AWS SigV4 and Ark bearer auth).
use super::Credentials;
use crate::protocol::{attempt::Call, error::Error};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(crate) fn ready(credentials: &Credentials) -> bool {
  credentials.access_key_id.as_deref().is_some_and(|v| !v.trim().is_empty())
    && credentials.secret_access_key.as_deref().is_some_and(|v| !v.trim().is_empty())
}

pub(crate) fn sign(
  call: &mut Call,
  credentials: &Credentials,
  region: &str,
  service: &str,
  now: u64,
) -> Result<(), Error> {
  if !ready(credentials) {
    return Err(Error::Build("Volcengine requires access_key_id and secret_access_key".into()));
  }
  let url =
    reqwest::Url::parse(&call.url).map_err(|_| Error::Build("invalid signing URL".into()))?;
  let date = super::sigv4::format_amz_date(now);
  let hash = hex::encode(Sha256::digest(&call.body));
  let mut headers = BTreeMap::from([
    (
      "host".to_owned(),
      format!(
        "{}{}",
        url.host().ok_or_else(|| Error::Build("missing signing host".into()))?,
        url.port().map(|p| format!(":{p}")).unwrap_or_default()
      ),
    ),
    ("x-date".to_owned(), date.clone()),
    ("x-content-sha256".to_owned(), hash.clone()),
  ]);
  for (key, value) in &call.headers {
    let key = key.to_ascii_lowercase();
    if key == "content-type" || key == "content-md5" {
      headers.insert(key, value.trim().to_owned());
    }
  }
  if let Some(token) = credentials.session_token.as_deref().filter(|v| !v.is_empty()) {
    headers.insert("x-security-token".into(), token.into());
  }
  let signed = headers.keys().cloned().collect::<Vec<_>>().join(";");
  let canonical_headers = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect::<String>();
  // Endpoint renders the actual query; canonicalization sorts encoded key/value pairs.
  let mut query: Vec<_> = url.query_pairs().map(|(k, v)| (encode(&k), encode(&v))).collect();
  query.sort();
  let query = query.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
  let method = match call.method {
    crate::protocol::attempt::Method::Get => "GET",
    crate::protocol::attempt::Method::Post => "POST",
  };
  let canonical =
    format!("{method}\n{}\n{query}\n{canonical_headers}\n{signed}\n{hash}", url.path());
  let scope = format!("{}/{region}/{service}/request", &date[..8]);
  let message =
    format!("HMAC-SHA256\n{date}\n{scope}\n{}", hex::encode(Sha256::digest(canonical.as_bytes())));
  let mut key = credentials.secret_access_key.as_deref().unwrap_or_default().as_bytes().to_vec();
  for part in [&date[..8], region, service, "request", &message] {
    key = mac(&key, part.as_bytes())?;
  }
  let auth = format!(
    "HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={}",
    credentials.access_key_id.as_deref().unwrap_or_default(),
    hex::encode(key)
  );
  call.headers.retain(|(k, _)| {
    !headers.contains_key(&k.to_ascii_lowercase()) && !k.eq_ignore_ascii_case("authorization")
  });
  call.headers.extend(headers);
  call.headers.push(("authorization".into(), auth));
  Ok(())
}
fn encode(value: &str) -> String {
  let mut result = String::new();
  for byte in value.bytes() {
    if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
      result.push(char::from(byte));
    } else {
      result.push_str(&format!("%{byte:02X}"));
    }
  }
  result
}
fn mac(key: &[u8], value: &[u8]) -> Result<Vec<u8>, Error> {
  let mut h =
    Hmac::<Sha256>::new_from_slice(key).map_err(|_| Error::Build("invalid signing key".into()))?;
  h.update(value);
  Ok(h.finalize().into_bytes().to_vec())
}
