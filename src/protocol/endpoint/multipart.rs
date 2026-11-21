//! In-memory bounded multipart encoding. The caller owns file limits and field semantics.
use crate::protocol::endpoint::Draft;
pub(crate) fn multipart_file(
  fields: &[(&str, String)],
  bytes: &[u8],
  mime: &str,
  extension: &str,
) -> Draft {
  let boundary = format!("wish-{}", uuid::Uuid::new_v4());
  let mut body = Vec::new();
  for (name, value) in fields {
    body.extend_from_slice(
      format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
        .as_bytes(),
    );
  }
  body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"input.{extension}\"\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
  body.extend_from_slice(bytes);
  body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
  Draft::post(
    None,
    vec![("content-type".into(), format!("multipart/form-data; boundary={boundary}"))],
    body,
  )
}
