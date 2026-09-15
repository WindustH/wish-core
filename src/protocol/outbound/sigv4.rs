//! AWS Signature Version 4: material proven by a signature rather than placed in a header.
//!
//! Bedrock does not take a key in a header - it takes a signature computed over the request as it
//! stands on the wire, which is why the signing happens at the moment a call is built: the path is
//! resolved, the body is rendered and the headers are merged, and only then is there something
//! complete enough to sign. What the account's material produces here is three headers -
//! `x-amz-date`, `authorization` and, for temporary credentials, `x-amz-security-token` - and the
//! secret itself never travels: it only derives the HMAC chain the signature is made with.
//!
//! Everything the chain needs out of a URL - the host, the path and the query - is split exactly
//! where AWS splits it, by the helpers below, so a caller of [`sign`] hands over the URL exactly as
//! it will be sent and nothing needs to agree with anything else.

use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// The material one AWS account signs with.
pub(crate) struct SigV4Credentials<'a> {
  /// The region the request is served from, which a signature's scope names.
  pub(crate) region: &'a str,
  /// The key id, which travels inside the `Authorization` header the signature builds.
  pub(crate) access_key_id: &'a str,
  /// The secret that derives the HMAC chain. It signs; it never travels.
  pub(crate) secret_access_key: &'a str,
  /// The token temporary credentials carry, which a signature names when there is one.
  pub(crate) session_token: Option<&'a str>,
}

/// The headers one signing produces, ready to be sent with the request that was signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SigV4Headers {
  /// `x-amz-date`: when the signature was made, `YYYYMMDDTHHMMSSZ`.
  pub(crate) x_amz_date: String,
  /// `authorization`: the `AWS4-HMAC-SHA256` header the service verifies.
  pub(crate) authorization: String,
  /// `x-amz-security-token`, for credentials that carry one.
  pub(crate) x_amz_security_token: Option<String>,
}

/// Signs one request as it stands on the wire.
///
/// The URL is split into the host, path and query AWS signs over, and `method`, `service` and
/// `body` complete what the signature covers. The steps in between are AWS's own: the canonical
/// request (method, path, query, the headers that are signed, the body's SHA-256), the string to
/// sign (algorithm, date, credential scope, the canonical request's SHA-256), the HMAC key chain
/// (secret, date, region, service), and the signature that chain makes of the string to sign. The
/// date is read from the clock, so every call is signed as of the moment it is made - an attempt
/// replaced later is signed again, never past its window.
///
/// # Errors
///
/// [`Err`] when the URL names no host to sign over, or an HMAC rejects its key - which RFC 2104
/// allows for no length, so the second never happens in practice.
pub(crate) fn sign(
  url: &str,
  method: &str,
  service: &str,
  body: &[u8],
  credentials: &SigV4Credentials<'_>,
) -> Result<SigV4Headers, String> {
  // The host a signature names: scheme stripped, port kept, nothing else.
  let host = url
    .strip_prefix("https://")
    .or_else(|| url.strip_prefix("http://"))
    .and_then(|rest| rest.split(['/', '?']).next())
    .filter(|host| !host.is_empty())
    .ok_or_else(|| format!("cannot derive a signing host from `{url}`"))?;
  // The path and the query, split where AWS splits it: from the first `/` after the host to the
  // `?`, the query exactly as rendered after it, empty when there is none.
  let no_scheme =
    url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or(url);
  let (before_query, query) = no_scheme.split_once('?').unwrap_or((no_scheme, ""));
  let path = match before_query.split_once('/') {
    Some((_, path)) => format!("/{path}"),
    None => String::from("/"),
  };
  // The moment a signature is dated, and the day its credential scope names: UTC seconds,
  // spelled the SigV4 way.
  let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |passed| passed.as_secs());
  let days = seconds / 86_400;
  let secs_of_day = seconds % 86_400;
  let (year, month, day) = civil_from_days(i64::try_from(days).unwrap_or(0));
  let amz_date = format!(
    "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
    secs_of_day / 3600,
    (secs_of_day % 3600) / 60,
    secs_of_day % 60
  );
  let date_stamp = amz_date[..8].to_owned();
  let payload_hash = hex::encode(Sha256::digest(body));

  // Canonical headers: host, x-amz-date, and the session token's header when the credentials carry
  // one. Other headers may travel unsigned - AWS requires only that the signed list names what the
  // signature actually covered.
  let session_token = credentials.session_token.map(str::trim).filter(|token| !token.is_empty());
  let mut signed_names = vec![String::from("host"), String::from("x-amz-date")];
  let mut canonical_headers = format!("host:{host}\nx-amz-date:{amz_date}\n");
  if let Some(token) = session_token {
    signed_names.push(String::from("x-amz-security-token"));
    canonical_headers.push_str("x-amz-security-token:");
    canonical_headers.push_str(token);
    canonical_headers.push('\n');
  }
  let signed_headers = signed_names.join(";");

  let canonical_request =
    format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}");
  let scope = format!("{date_stamp}/{}/{service}/aws4_request", credentials.region);
  let string_to_sign = format!(
    "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
    hex::encode(Sha256::digest(canonical_request.as_bytes()))
  );

  let k_date =
    hmac_key(format!("AWS4{}", credentials.secret_access_key).as_bytes(), date_stamp.as_bytes())?;
  let k_region = hmac_key(&k_date, credentials.region.as_bytes())?;
  let k_service = hmac_key(&k_region, service.as_bytes())?;
  let k_signing = hmac_key(&k_service, b"aws4_request")?;
  let signature = hex::encode(hmac_key(&k_signing, string_to_sign.as_bytes())?);

  Ok(SigV4Headers {
    x_amz_date: amz_date,
    authorization: format!(
      "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
      credentials.access_key_id
    ),
    x_amz_security_token: session_token.map(str::to_owned),
  })
}

/// HMAC-SHA256 accepts keys of any length (RFC 2104), so the error is a formality kept for the
/// day a caller derives keys some other way.
fn hmac_key(key: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
  let mut mac = Hmac::<Sha256>::new_from_slice(key)
    .map_err(|error| format!("an HMAC rejected its key: {error}"))?;
  mac.update(data);
  Ok(mac.finalize().into_bytes().to_vec())
}

/// Days since the epoch to `(year, month, day)`: Howard Hinnant's civil-from-days, the one calendar
/// a timestamp needs.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
  let z = days + 719_468;
  let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
  let doe = z - era * 146_097;
  let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
  let year = yoe + era * 400;
  let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  let mp = (5 * doy + 2) / 153;
  let day = doy - (153 * mp + 2) / 5 + 1;
  let month = if mp < 10 { mp + 3 } else { mp - 9 };
  (if month <= 2 { year + 1 } else { year }, month, day)
}
